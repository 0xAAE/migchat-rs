use crate::proto::chat_room_service_client::ChatRoomServiceClient;
use crate::proto::{Chat, ChatInfo, Invitation, Post, Registration, User, UserInfo};
use crate::Event;

use config::Config;
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
    Registered(u64),        // user_id
    UserEntered(User),      // user_id, name, short_name
    UserGone(u64),          // user_id
    ChatUpdated(Chat),      // chat_id
    ChatDeleted(u32),       // chat_id
    Invitation(Invitation), // user_id, chat_id
    NewPost(Post),          // chat_id, user_id, text, [attachments]
}

pub enum Command {
    CreateChat(ChatInfo), // create new chat
    Invite(Invitation),   // invite user to chat
    Post(Post),           // send new post
    Exit,                 // exit chat room
}

pub struct MigchatClient {
    user: User,
    rx_command: mpsc::Receiver<Command>,
    test_users: Option<Vec<User>>,
}

impl MigchatClient {
    pub fn new(settings: &Config, rx_command: mpsc::Receiver<Command>) -> Self {
        let name = settings.get_str("name").unwrap_or("Anonimous".to_string());
        let short_name = settings.get_str("short_name").unwrap_or("Nemo".to_string());
        // if there are test users in settings, create them
        let test_users = if let Ok(config_test_users) = settings.get_array("test_users") {
            let mut next_id = 100;
            let mut test_users = Vec::new();
            for item in config_test_users {
                if let Ok(names) = item.into_array() {
                    if names.len() == 2 {
                        test_users.push(User {
                            user_id: next_id,
                            name: names[0].to_string(),
                            short_name: names[1].to_string(),
                        });
                        next_id += 1;
                    }
                }
            }
            Some(test_users)
        } else {
            None
        };

        MigchatClient {
            user: User {
                user_id: 0,
                name,
                short_name,
            },
            rx_command,
            test_users,
        }
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

        if let Some(test_users) = self.test_users.take() {
            for u in test_users {
                if let Err(e) = tx_event
                    .send(Event::Client(ChatRoomEvent::UserEntered(u)))
                    .await
                {
                    error!("failed send test user: {}", e);
                }
            }
        }

        let mut client = ChatRoomServiceClient::new(channel);

        // register
        let user_info = UserInfo {
            name: self.user.name.clone(),
            short_name: self.user.short_name.clone(),
        };
        info!("registering as {:?}", user_info);
        let reg_req = tonic::Request::new(user_info);
        if let Ok(reg_res) = client.register(reg_req).await {
            self.user.user_id = reg_res.into_inner().user_id;
            info!("logged as {:?}", self.user);
            if let Err(e) = tx_event
                .send(Event::Client(ChatRoomEvent::Registered(self.user.user_id)))
                .await
            {
                error!("failed to translate own user_id to UI");
            }
            // launch accepting users in separate task
            let fut = MigchatClient::read_users_stream(
                client.clone(),
                tx_event.clone(),
                self.user.user_id,
            );
            tokio::spawn(fut);
            // launch accepting invitations in separate task
            let fut = MigchatClient::read_invitations_stream(
                client.clone(),
                tx_event.clone(),
                self.user.user_id,
            );
            tokio::spawn(fut);
            // launch accepting chats in separate task
            let fut = MigchatClient::read_chats_stream(
                client.clone(),
                tx_event.clone(),
                self.user.user_id,
            );
            tokio::spawn(fut);
            // launch accepting posts in separate task
            let fut = MigchatClient::read_posts_stream(
                client.clone(),
                tx_event.clone(),
                self.user.user_id,
            );
            tokio::spawn(fut);
        } else {
            warn!("registration failed");
        }

        // start command loop
        loop {
            match tokio::time::timeout(Duration::from_millis(500), self.rx_command.recv()).await {
                Err(_) => {
                    // timeout, test exit flag and recv commands
                    if exit_flag.load(Ordering::SeqCst) {
                        break;
                    }
                }
                Ok(command) => match command {
                    Some(command) => match command {
                        Command::CreateChat(info) => {
                            assert_eq!(info.user_id, self.user.user_id);
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
                        Command::Invite(_invitation) => {
                            error!("inviting others is not implemented yet");
                        }
                        Command::Post(post) => {
                            assert_eq!(post.user_id, self.user.user_id);
                            match client.create_post(post).await {
                                Ok(response) => {
                                    debug!("send post: {:?}", response.into_inner());
                                }
                                Err(e) => {
                                    warn!("failed to create chat: {}", e);
                                }
                            }
                        }
                        Command::Exit => {
                            error!("exitting chat room is not implemented yet");
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
        user_id: u64,
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
                                .send(Event::Client(ChatRoomEvent::UserGone(user.user_id)))
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
        user_id: u64,
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
        user_id: u64,
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
        user_id: u64,
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
                                .send(Event::Client(ChatRoomEvent::ChatDeleted(chat.chat_id)))
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
