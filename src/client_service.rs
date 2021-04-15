use crate::proto::chat_room_service_client::ChatRoomServiceClient;
use crate::proto::{
    Chat, ChatId, ChatInfo, ChatReference, Invitation, Post, Registration, User, UserId, UserInfo,
};
use crate::Event;

use log::{debug, error, info, warn};
use std::{
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::Duration,
};
use tokio::sync::mpsc;
use tonic::transport::{Channel, Endpoint};

pub enum ChatRoomEvent {
    Registered(UserId),     // user_id
    UserEntered(User),      // user_id, name, short_name
    UserGone(UserId),       // user_id
    ChatUpdated(Chat),      // chat_id
    ChatDeleted(ChatId),    // chat_id
    Invitation(Invitation), // user_id, chat_id
    NewPost(Post),          // chat_id, user_id, text, [attachments]
}

pub enum Command {
    Register(UserInfo),   //register on server
    CreateChat(ChatInfo), // create new chat
    Invite(Invitation),   // invite user to chat
    EnterChat(ChatId),    // enter chat specified
    Post(Post),           // send new post
    Exit,                 // exit chat room
}

pub struct MigchatClient {
    rx_command: mpsc::Receiver<Command>,
}

impl MigchatClient {
    pub fn new(rx_command: mpsc::Receiver<Command>) -> Self {
        MigchatClient { rx_command }
    }

    pub async fn launch(
        &mut self,
        tx_event: mpsc::Sender<Event>,
        exit_flag: Arc<AtomicBool>,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let channel = Endpoint::from_static("http://0.0.0.0:50051")
            .timeout(Duration::from_secs(10))
            .connect()
            .await?;

        let mut client = ChatRoomServiceClient::new(channel);

        // wait registartion info from App/UI
        let mut user_info = UserInfo::default();
        while let Some(command) = self.rx_command.recv().await {
            match command {
                Command::Register(info) => {
                    if !info.name.is_empty() || !info.short_name.is_empty() {
                        user_info = info;
                        break;
                    }
                }
                Command::Exit => {
                    info!("exit requested, proceed");
                    if let Err(e) = tx_event.send(Event::Exit).await {
                        error!("failed routing exit event chat: {}", e);
                    }
                    return Ok(());
                }
                _ => {
                    if exit_flag.load(Ordering::Relaxed) {
                        info!("exitting before registration info received");
                        return Ok(());
                    }
                }
            }
        }

        // register
        info!("logging as {}", &user_info);
        let reg_req = tonic::Request::new(user_info);
        let user_id: UserId = if let Ok(reg_res) = client.register(reg_req).await {
            let user_id = reg_res.into_inner().user_id;
            info!("logged successfully");
            if let Err(e) = tx_event
                .send(Event::Client(ChatRoomEvent::Registered(user_id)))
                .await
            {
                error!("failed to translate own user_id to UI: {}", e);
            }
            // launch accepting users in separate task
            let fut = MigchatClient::read_users_stream(client.clone(), tx_event.clone(), user_id);
            tokio::spawn(fut);
            // launch accepting invitations in separate task
            let fut =
                MigchatClient::read_invitations_stream(client.clone(), tx_event.clone(), user_id);
            tokio::spawn(fut);
            // launch accepting chats in separate task
            let fut = MigchatClient::read_chats_stream(client.clone(), tx_event.clone(), user_id);
            tokio::spawn(fut);
            // launch accepting posts in separate task
            let fut = MigchatClient::read_posts_stream(client.clone(), tx_event.clone(), user_id);
            tokio::spawn(fut);
            user_id
        } else {
            warn!("registration failed");
            return Err(Box::new(ClientServiceError {
                text: String::from("failed to register on server"),
            }));
        };

        // start command loop
        loop {
            match tokio::time::timeout(Duration::from_millis(500), self.rx_command.recv()).await {
                Err(_) => {
                    // timeout, test exit flag and recv commands
                    if exit_flag.load(Ordering::Relaxed) {
                        break;
                    }
                }
                Ok(command) => match command {
                    Some(command) => match command {
                        Command::CreateChat(info) => {
                            assert_eq!(info.user_id, user_id);
                            match client.create_chat(info).await {
                                Ok(response) => {
                                    if let Err(e) = tx_event
                                        .send(Event::Client(ChatRoomEvent::ChatUpdated(
                                            response.into_inner(),
                                        )))
                                        .await
                                    {
                                        error!("failed routing created chat: {}", e);
                                    }
                                }
                                Err(e) => {
                                    warn!("failed to create chat: {}", e);
                                }
                            }
                        }
                        Command::Invite(invitation) => match client.invite_user(invitation).await {
                            Ok(response) => {
                                debug!("invite user: {:?}", response.into_inner());
                            }
                            Err(e) => {
                                warn!("failed to invite user: {}", e);
                            }
                        },
                        Command::Post(post) => {
                            assert_eq!(post.user_id, user_id);
                            match client.create_post(post).await {
                                Ok(response) => {
                                    debug!("send post: {:?}", response.into_inner());
                                }
                                Err(e) => {
                                    warn!("failed to create chat: {}", e);
                                }
                            }
                        }
                        Command::EnterChat(chat_id) => {
                            match client.enter_chat(ChatReference { user_id, chat_id }).await {
                                Ok(response) => {
                                    debug!("send post: {:?}", response.into_inner());
                                }
                                Err(e) => {
                                    warn!("failed to create chat: {}", e);
                                }
                            }
                        }
                        Command::Exit => {
                            match client.logout(Registration { user_id }).await {
                                Ok(response) => {
                                    debug!("logout: {:?}", response.into_inner());
                                }
                                Err(e) => {
                                    warn!("failed to logout: {}", e);
                                }
                            }
                            if let Err(e) = tx_event.send(Event::Exit).await {
                                error!("failed routing exit event chat: {}", e);
                            }
                        }
                        Command::Register(_) => {
                            warn!("user has alredy registered");
                        }
                    },
                    None => {
                        info!("command channel has closed by receiver");
                        break;
                    }
                },
            }
        }
        info!("exitting, bye!");
        Ok(())
    }

    async fn read_users_stream(
        client: ChatRoomServiceClient<Channel>,
        tx_event: mpsc::Sender<Event>,
        user_id: UserId,
    ) {
        let mut client = client;
        match client
            .get_users(tonic::Request::new(Registration { user_id }))
            .await
        {
            Ok(response) => {
                let mut stream = response.into_inner();
                while let Some(update_users) = stream.message().await.ok().flatten() {
                    if !update_users.added.is_empty() {
                        for user in update_users.added {
                            debug!("user entered: {:?}", &user);
                            if let Err(e) = tx_event
                                .send(Event::Client(ChatRoomEvent::UserEntered(user)))
                                .await
                            {
                                error!("failed to transfer entered user: {}", e);
                            }
                        }
                    }
                    if !update_users.gone.is_empty() {
                        for user in update_users.gone {
                            debug!("user exited: {:?}", &user);
                            if let Err(e) = tx_event
                                .send(Event::Client(ChatRoomEvent::UserGone(user.id)))
                                .await
                            {
                                error!("failed to transfer exited user: {}", e);
                            }
                        }
                    }
                }
            }
            Err(e) => {
                warn!("no more updated users: {}", e);
            }
        }
    }

    async fn read_invitations_stream(
        client: ChatRoomServiceClient<Channel>,
        tx_event: mpsc::Sender<Event>,
        user_id: UserId,
    ) {
        let mut client = client;
        match client
            .get_invitations(tonic::Request::new(Registration { user_id }))
            .await
        {
            Ok(response) => {
                let mut stream = response.into_inner();
                while let Some(invitation) = stream.message().await.ok().flatten() {
                    debug!("new invitation: {:?}", &invitation);
                    if let Err(e) = tx_event
                        .send(Event::Client(ChatRoomEvent::Invitation(invitation)))
                        .await
                    {
                        error!("failed to transfer invitation to UI {}", e);
                    }
                }
            }
            Err(e) => {
                warn!("no more invitations: {}", e);
            }
        }
    }

    async fn read_posts_stream(
        client: ChatRoomServiceClient<Channel>,
        tx_event: mpsc::Sender<Event>,
        user_id: UserId,
    ) {
        let mut client = client;
        match client
            .get_posts(tonic::Request::new(Registration { user_id }))
            .await
        {
            Ok(response) => {
                let mut stream = response.into_inner();
                while let Some(post) = stream.message().await.ok().flatten() {
                    debug!("new post: {:?}", &post);
                    if let Err(e) = tx_event
                        .send(Event::Client(ChatRoomEvent::NewPost(post)))
                        .await
                    {
                        error!("failed to transfer post to UI {}", e);
                    }
                }
            }
            Err(e) => {
                warn!("no more posts: {}", e);
            }
        }
    }

    async fn read_chats_stream(
        client: ChatRoomServiceClient<Channel>,
        tx_event: mpsc::Sender<Event>,
        user_id: UserId,
    ) {
        let mut client = client;
        match client
            .get_chats(tonic::Request::new(Registration { user_id }))
            .await
        {
            Ok(response) => {
                let mut stream = response.into_inner();
                while let Some(updated_chats) = stream.message().await.ok().flatten() {
                    if !updated_chats.added.is_empty() {
                        for chat in updated_chats.added {
                            debug!("chat updated: {:?}", &chat);
                            if let Err(e) = tx_event
                                .send(Event::Client(ChatRoomEvent::ChatUpdated(chat)))
                                .await
                            {
                                error!("failed to transfer updated chat: {}", e);
                            }
                        }
                    }
                    if !updated_chats.gone.is_empty() {
                        for chat in updated_chats.gone {
                            debug!("chat has gone: {:?}", &chat);
                            if let Err(e) = tx_event
                                .send(Event::Client(ChatRoomEvent::ChatDeleted(chat.id)))
                                .await
                            {
                                error!("failed to transfer deleted chat: {}", e);
                            }
                        }
                    }
                }
            }
            Err(e) => {
                warn!("no more updated chats: {}", e);
            }
        }
    }
}

#[derive(Debug)]
struct ClientServiceError {
    text: String,
}

impl std::error::Error for ClientServiceError {}

impl std::fmt::Display for ClientServiceError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.text)
    }
}
