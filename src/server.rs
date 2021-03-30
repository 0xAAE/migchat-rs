use env_logger::{fmt::TimestampPrecision, Builder, Env, Target};
use futures::Stream; //, StreamExt};
use fxhash::hash64;
use log::{debug, error, info};
use std::{
    collections::HashMap,
    ops::Deref,
    pin::Pin,
    sync::{
        atomic::{AtomicU32, Ordering},
        Arc, RwLock,
    },
};
use tokio::sync::mpsc;
use tonic::{transport::Server, Request, Response, Status};

mod proto;
//mod user;

use proto::chat_room_service_server::{ChatRoomService, ChatRoomServiceServer};
use proto::{
    Chat, ChatInfo, Invitation, Post, Registration, RequestChats, RequestInvitations, RequestUsers,
    Result as RpcResult, UpdateChats, UpdateUsers, User, UserInfo,
};
//use user::User;

#[derive(Debug, Default)]
pub struct ChatRoomImpl {
    next_chat_id: AtomicU32,
    // user_id -> user info:
    users: RwLock<HashMap<u64, User>>,
    // chat_id -> chat info:
    chats: RwLock<HashMap<u32, proto::Chat>>,
    //
    // listeners, all are linked to the appropriate user_id
    //
    // new users:
    users_listeners: RwLock<HashMap<u64, mpsc::Sender<Arc<User>>>>,
    // new invitations:
    invitations_listeners: RwLock<HashMap<u64, mpsc::Sender<Invitation>>>,
    // new chats:
    chats_listeners: RwLock<HashMap<u64, mpsc::Sender<Arc<Chat>>>>,
}

impl ChatRoomImpl {}

#[tonic::async_trait]
impl ChatRoomService for ChatRoomImpl {
    #[doc = " Sends a reqistration request"]
    async fn register(&self, request: Request<UserInfo>) -> Result<Response<Registration>, Status> {
        debug!("register(): {:?}", &request);
        let user_info = request.into_inner();
        let user_id = hash64(&user_info);
        let new_user = User {
            user_id,
            name: user_info.name,
            short_name: user_info.short_name,
        };
        let mut send_list = Vec::new();
        if let Ok(listeners) = self.users_listeners.read() {
            for listener in listeners.values() {
                send_list.push(listener.clone());
            }
        }
        if !send_list.is_empty() {
            let send_user = Arc::new(new_user.clone());
            for tx in send_list {
                if let Err(e) = tx.send(send_user.clone()).await {
                    error!("failed to broadcast new user: {}", e);
                    //no break;
                }
            }
        }
        if let Ok(mut locked) = self.users.write() {
            let _ = locked.insert(user_id, new_user);
            Ok(Response::new(Registration { user_id }))
        } else {
            Err(tonic::Status::internal("no access to registration info"))
        }
    }

    #[doc = "Server streaming response type for the GetInvitations method."]
    type GetInvitationsStream =
        Pin<Box<dyn Stream<Item = Result<Invitation, tonic::Status>> + Send + Sync + 'static>>;

    #[doc = " Asks for invitations"]
    async fn get_invitations(
        &self,
        request: tonic::Request<RequestInvitations>,
    ) -> Result<tonic::Response<Self::GetInvitationsStream>, tonic::Status> {
        // get source channel of invitations
        debug!("get_invitations(): {:?}", &request);
        let user_id = request.into_inner().user_id;
        let rx_invit = if let Ok(mut locked) = self.invitations_listeners.write() {
            let (tx_invit, rx_invit) = mpsc::channel(4);
            locked.insert(user_id, tx_invit);
            rx_invit
        } else {
            return Err(tonic::Status::internal("no access to invitation channel"));
        };
        // launch stream source
        let (tx, rx) = mpsc::channel(4);
        tokio::spawn(async move {
            let mut rx_invit = rx_invit;
            while let Some(invitation) = rx_invit.recv().await {
                tx.send(Ok(invitation)).await.unwrap();
            }
            debug!("stop streaming invitations to user-{}", user_id);
        });
        Ok(Response::new(Box::pin(
            tokio_stream::wrappers::ReceiverStream::new(rx),
        )))
    }

    #[doc = " Sends a logout request using the first invitation from the server"]
    async fn logout(
        &self,
        request: tonic::Request<Registration>,
    ) -> Result<tonic::Response<RpcResult>, tonic::Status> {
        debug!("logout(): {:?}", &request);
        let user_id = request.into_inner().user_id;
        if let Ok(mut listeners) = self.users_listeners.write() {
            let _ = listeners.remove(&user_id);
        } else {
            error!("failed locking users listeners (logout)");
        }
        if let Ok(mut listeners) = self.chats_listeners.write() {
            let _ = listeners.remove(&user_id);
        } else {
            error!("failed locking chats listeners (logout)");
        }
        if let Ok(mut listeners) = self.invitations_listeners.write() {
            let _ = listeners.remove(&user_id);
        } else {
            error!("failed locking invitations listeners (logout)");
        }
        if let Ok(mut users) = self.users.write() {
            let _ = users.remove(&user_id);
        } else {
            error!("failed locking users (logout)");
        }
        Ok(Response::new(RpcResult {
            ok: true,
            description: "logout successful".to_string(),
        }))
    }

    #[doc = "Server streaming response type for the StartChating method."]
    type StartChatingStream =
        Pin<Box<dyn Stream<Item = Result<Post, tonic::Status>> + Send + Sync + 'static>>;

    #[doc = " Starts chating"]
    async fn start_chating(
        &self,
        request: tonic::Request<tonic::Streaming<Post>>,
    ) -> Result<tonic::Response<Self::StartChatingStream>, tonic::Status> {
        debug!("start_chating(): {:?}", &request);
        Err(tonic::Status::unimplemented(
            "start_chating() is not implemented yet",
        ))
    }

    #[doc = "Server streaming response type for the GetUsers method."]
    type GetUsersStream =
        Pin<Box<dyn Stream<Item = Result<UpdateUsers, tonic::Status>> + Send + Sync + 'static>>;

    #[doc = " Asks for contacts list"]
    async fn get_users(
        &self,
        request: tonic::Request<RequestUsers>,
    ) -> Result<tonic::Response<Self::GetUsersStream>, tonic::Status> {
        debug!("get_users(): {:?}", &request);
        let user_id = request.into_inner().user_id;
        let (listener, notifier) = mpsc::channel::<Arc<User>>(4);
        if let Ok(mut listeners) = self.users_listeners.write() {
            listeners.insert(user_id, listener.clone());
        } else {
            return Err(tonic::Status::internal("no access to users listeners"));
        }
        // collect existing users
        let mut existing = Vec::new();
        if let Ok(users) = self.users.read() {
            for user in users.values() {
                if user.user_id != user_id {
                    existing.push(user.clone());
                }
            }
        } else {
            error!("failed to read existing users");
        }
        // start permanent listener that streams data to remote client
        let (tx, rx) = mpsc::channel(4);
        tokio::spawn(async move {
            if !existing.is_empty() {
                // send existing users
                let start_update = UpdateUsers {
                    added: existing,
                    gone: Vec::new(),
                };
                debug!(
                    "sending {} existing users to user-{}",
                    start_update.added.len(),
                    user_id
                );
                if let Err(e) = tx.send(Ok(start_update)).await {
                    error!("failed sending existing users: {}", e);
                }
            }
            // re-translate new users
            let mut notifier = notifier;
            while let Some(user) = notifier.recv().await {
                debug!("re-translating new user to user-{}", user_id);
                let update = UpdateUsers {
                    added: vec![user.deref().clone()],
                    gone: Vec::new(),
                };
                if let Err(e) = tx.send(Ok(update)).await {
                    error!("failed sending users update: {}, stop", e);
                    break;
                }
            }
            debug!("stop streaming users to user-{}", user_id);
        });
        // start streaming activity, data consumer
        Ok(Response::new(Box::pin(
            tokio_stream::wrappers::ReceiverStream::new(rx),
        )))
    }

    #[doc = "Server streaming response type for the GetChats method."]
    type GetChatsStream =
        Pin<Box<dyn Stream<Item = Result<UpdateChats, tonic::Status>> + Send + Sync + 'static>>;

    #[doc = " Asks for chats list"]
    async fn get_chats(
        &self,
        request: tonic::Request<RequestChats>,
    ) -> Result<tonic::Response<Self::GetChatsStream>, tonic::Status> {
        debug!("get_chats(): {:?}", &request);
        let user_id = request.into_inner().user_id;
        let (listener, notifier) = mpsc::channel::<Arc<Chat>>(4);
        if let Ok(mut listeners) = self.chats_listeners.write() {
            listeners.insert(user_id, listener.clone());
        } else {
            // failed locking listeners
            return Err(tonic::Status::internal("no access to chat listeners"));
        }
        // collect existing chats
        let mut existing = Vec::new();
        if let Ok(chats) = self.chats.read() {
            for chat in chats.values() {
                existing.push(chat.clone());
            }
        } else {
            error!("failed to read existing chats");
        }
        // start permanent listener that streams data to remote client
        let (tx, rx) = mpsc::channel(4);
        tokio::spawn(async move {
            if !existing.is_empty() {
                // send existing chats
                let start_update = UpdateChats {
                    added: existing,
                    gone: Vec::new(),
                };
                debug!(
                    "sending {} existing chats to user-{}",
                    start_update.added.len(),
                    user_id
                );
                if let Err(e) = tx.send(Ok(start_update)).await {
                    error!("failed sending existing chats: {}", e);
                }
            }
            // re-translate new chats
            let mut notifier = notifier;
            while let Some(chat) = notifier.recv().await {
                debug!("re-translating new chat to user-{}", user_id);
                let update = UpdateChats {
                    added: vec![(*chat).clone()],
                    gone: Vec::new(),
                };
                if let Err(e) = tx.send(Ok(update)).await {
                    error!("failed sending chat: {}", e);
                    break;
                }
            }
            debug!("stop streaming chats to user-{}", user_id);
        });
        // start streaming activity, data consumer
        Ok(Response::new(Box::pin(
            tokio_stream::wrappers::ReceiverStream::new(rx),
        )))
    }

    #[doc = " Creates new chat"]
    async fn create_chat(
        &self,
        request: tonic::Request<ChatInfo>,
    ) -> Result<tonic::Response<Chat>, tonic::Status> {
        debug!("create_chat(): {:?}", &request);
        let info = request.get_ref();
        let users = if info.auto_enter {
            vec![info.user_id]
            // todo: auto include desired users
        } else {
            Vec::new()
        };
        let chat = proto::Chat {
            chat_id: self.next_chat_id.fetch_add(1, Ordering::SeqCst),
            description: info.description.clone(),
            users,
        };
        if let Ok(mut chats) = self.chats.write() {
            chats.insert(chat.chat_id, chat.clone());
        } else {
            return Err(tonic::Status::internal("failed to access chats"));
        }
        let mut send_list = Vec::new();
        if let Ok(listeners) = self.chats_listeners.read() {
            for listener in listeners.values() {
                send_list.push(listener.clone());
            }
        }
        if !send_list.is_empty() {
            let send_chat = Arc::new(chat.clone());
            for tx in send_list {
                if let Err(e) = tx.send(send_chat.clone()).await {
                    error!("failed to broadcast new chat: {}", e);
                    // no break;
                }
            }
        }
        Ok(Response::new(chat))
    }

    #[doc = " Invites user to chat"]
    async fn invite_user(
        &self,
        request: tonic::Request<Invitation>,
    ) -> Result<tonic::Response<RpcResult>, tonic::Status> {
        debug!("invite_user(): {:?}", &request);
        let invitation = request.into_inner();
        // test chat exists
        if let Ok(chats) = self.chats.read() {
            if !chats.contains_key(&invitation.chat_id) {
                return Err(tonic::Status::not_found(format!(
                    "chat {} does not exist",
                    invitation.chat_id
                )));
            }
        } else {
            return Err(tonic::Status::internal("failed read chats"));
        }
        // test recepient exists
        if let Ok(users) = self.users.read() {
            if !users.contains_key(&invitation.to_user_id) {
                return Err(tonic::Status::not_found(format!(
                    "user-{} is not registered",
                    invitation.to_user_id
                )));
            }
        } else {
            return Err(tonic::Status::internal("failed read users"));
        }
        // try to get send channel and send invitation
        let tx = if let Ok(listeners) = self.invitations_listeners.read() {
            if let Some(tx) = listeners.get(&invitation.to_user_id) {
                tx.clone()
            } else {
                return Err(tonic::Status::not_found(format!(
                    "user-{} did not subscribe to invitations",
                    invitation.to_user_id
                )));
            }
        } else {
            return Err(tonic::Status::internal(
                "failed read invitation subscribers",
            ));
        };
        if let Err(e) = tx.send(invitation).await {
            error!("failed to send invitation: {}", e);
            Err(tonic::Status::internal("failed to send invitation"))
        } else {
            Ok(Response::new(RpcResult {
                ok: true,
                description: "invitation has been sent".to_string(),
            }))
        }
    }

    #[doc = " Enters the chat"]
    async fn enter_chat(
        &self,
        request: tonic::Request<Chat>,
    ) -> Result<tonic::Response<RpcResult>, tonic::Status> {
        debug!("enter_chat(): {:?}", &request);
        Err(tonic::Status::unimplemented(
            "enter_chat() is not implemented yet",
        ))
    }

    #[doc = " Leaves active chat"]
    async fn leave_chat(
        &self,
        request: tonic::Request<Chat>,
    ) -> Result<tonic::Response<RpcResult>, tonic::Status> {
        debug!("leave_chat(): {:?}", &request);
        Err(tonic::Status::unimplemented(
            "leave_chat() is not implemented yet",
        ))
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    Builder::from_env(Env::default().default_filter_or("debug,h2=info,tower=info,hyper=info"))
        .target(Target::Stdout)
        .format_timestamp(Some(TimestampPrecision::Seconds))
        .init();

    let addr = "0.0.0.0:50051".parse().unwrap();
    let chat_room = ChatRoomImpl::default();
    info!("Chat room is listening on {}", addr);

    let svc = ChatRoomServiceServer::new(chat_room);
    Server::builder().add_service(svc).serve(addr).await?;

    Ok(())
}
