use env_logger::{fmt::TimestampPrecision, Builder, Env, Target};
use futures::Stream; //, StreamExt};
use log::{debug, info};
use std::collections::HashMap;
use std::pin::Pin;
use std::sync::{
    atomic::{AtomicU32, Ordering},
    Arc, Mutex, RwLock,
};
use tokio::sync::mpsc;
use tonic::{transport::Server, Request, Response, Status};
use uuid::Uuid;

mod proto;
mod user;

use proto::chat_room_service_server::{ChatRoomService, ChatRoomServiceServer};
use proto::{
    Chat, ChatInfo, Invitation, Post, Registration, RequestChats, RequestInvitations, RequestUsers,
    Result as RpcResult, Session, UpdateChats, UpdateUsers, UserInfo,
};
use user::User;

#[derive(Debug, Default)]
pub struct ChatRoomImpl {
    next_id: AtomicU32,
    users: RwLock<HashMap<proto::Uuid, User>>,
    users_listeners: Mutex<Vec<mpsc::Sender<Arc<User>>>>,
    invitations: Mutex<HashMap<u32, mpsc::Sender<Invitation>>>,
}

impl ChatRoomImpl {
    #[allow(dead_code)]
    fn notify_added(&self, u: Arc<User>) {
        if let Ok(mut locked) = self.users_listeners.lock() {
            locked.retain(|l| {
                // return false to remove listener from the vec!
                l.blocking_send(u.clone()).is_ok()
            });
        }
    }
}

#[tonic::async_trait]
impl ChatRoomService for ChatRoomImpl {
    #[doc = " Sends a reqistration request"]
    async fn register(&self, request: Request<UserInfo>) -> Result<Response<Registration>, Status> {
        debug!("register: {:?}", request);
        let uuid_val = proto::Uuid {
            value: format!("{}", Uuid::new_v4()),
        };
        if let Ok(mut locked) = self.users.write() {
            let _ = locked.insert(
                uuid_val.clone(),
                User {
                    id: self.next_id.fetch_add(1, Ordering::SeqCst),
                    name: request.get_ref().name.to_string(),
                    short_name: request.get_ref().short_name.to_string(),
                },
            );
            // Send back our formatted greeting
            Ok(Response::new(Registration {
                uuid: Some(uuid_val),
            }))
        } else {
            Err(tonic::Status::internal("no access to registration info"))
        }
    }

    #[doc = " Sends a login, gets invitations. The first invitation has chat_id = 0 and own user_id"]
    async fn login(
        &self,
        request: tonic::Request<Registration>,
    ) -> Result<tonic::Response<Session>, tonic::Status> {
        debug!("login: {:?}", request);
        if let Ok(locked) = self.users.read() {
            if let Some(ref uuid) = request.get_ref().uuid {
                if let Some(user) = locked.get(uuid) {
                    Ok(Response::new(Session { id: user.id }))
                } else {
                    debug!("user is not registered: {:?}", request);
                    Err(tonic::Status::permission_denied("user is not registered"))
                }
            } else {
                debug!("no user UUID provided in {:?}", request);
                Err(tonic::Status::invalid_argument("no user UUID provided"))
            }
        } else {
            debug!("no access to registration info");
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
        debug!("get_invitations: {:?}", request);
        let rx_invit = if let Ok(mut locked) = self.invitations.lock() {
            let (tx_invit, rx_invit) = mpsc::channel(4);
            locked.insert(request.into_inner().session_id, tx_invit);
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
        });
        Ok(Response::new(Box::pin(
            tokio_stream::wrappers::ReceiverStream::new(rx),
        )))
    }

    #[doc = " Sends a logout request using the first invitation from the server"]
    async fn logout(
        &self,
        _request: tonic::Request<Session>,
    ) -> Result<tonic::Response<RpcResult>, tonic::Status> {
        Err(tonic::Status::unimplemented(
            "logout() is not implemented yet",
        ))
    }

    #[doc = "Server streaming response type for the StartChating method."]
    type StartChatingStream =
        Pin<Box<dyn Stream<Item = Result<Post, tonic::Status>> + Send + Sync + 'static>>;

    #[doc = " Starts chating"]
    async fn start_chating(
        &self,
        _request: tonic::Request<tonic::Streaming<Post>>,
    ) -> Result<tonic::Response<Self::StartChatingStream>, tonic::Status> {
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
        _request: tonic::Request<RequestUsers>,
    ) -> Result<tonic::Response<Self::GetUsersStream>, tonic::Status> {
        if let Ok(mut listeners) = self.users_listeners.lock() {
            let (listener, mut notifier) = mpsc::channel::<Arc<User>>(4);

            // start permanent listener, streaming data producer
            let (tx, rx) = mpsc::channel(4);
            tokio::spawn(async move {
                while let Some(user) = notifier.recv().await {
                    let update = UpdateUsers {
                        added: vec![user.make_proto()],
                        gone: Vec::new(),
                    };
                    tx.send(Ok(update)).await.unwrap();
                }
            });

            // send existing users
            if let Ok(users) = self.users.read() {
                for user in users.values() {
                    let u = Arc::new(user.clone());
                    let tx = listener.clone();
                    tokio::spawn(async move {
                        let _ = tx.send(u).await;
                    });
                }
            } else {
                // failed read-locking users
            }

            listeners.push(listener);

            // start streaming activity, data consumer
            Ok(Response::new(Box::pin(
                tokio_stream::wrappers::ReceiverStream::new(rx),
            )))
        } else {
            // failed locking listeners
            Err(tonic::Status::internal("no access to listeners"))
        }
    }

    #[doc = "Server streaming response type for the GetChats method."]
    type GetChatsStream =
        Pin<Box<dyn Stream<Item = Result<UpdateChats, tonic::Status>> + Send + Sync + 'static>>;

    #[doc = " Asks for chats list"]
    async fn get_chats(
        &self,
        _request: tonic::Request<RequestChats>,
    ) -> Result<tonic::Response<Self::GetChatsStream>, tonic::Status> {
        Err(tonic::Status::unimplemented(
            "get_chats() is not implemented yet",
        ))
    }

    #[doc = " Creates new chat"]
    async fn create_chat(
        &self,
        _request: tonic::Request<ChatInfo>,
    ) -> Result<tonic::Response<Chat>, tonic::Status> {
        Err(tonic::Status::unimplemented(
            "create_chat() is not implemented yet",
        ))
    }

    #[doc = " Invites user to chat"]
    async fn invite_user(
        &self,
        _request: tonic::Request<Invitation>,
    ) -> Result<tonic::Response<RpcResult>, tonic::Status> {
        Err(tonic::Status::unimplemented(
            "invite_user() is not implemented yet",
        ))
    }

    #[doc = " Enters the chat"]
    async fn enter_chat(
        &self,
        _request: tonic::Request<Chat>,
    ) -> Result<tonic::Response<RpcResult>, tonic::Status> {
        Err(tonic::Status::unimplemented(
            "enter_chat() is not implemented yet",
        ))
    }

    #[doc = " Leaves active chat"]
    async fn leave_chat(
        &self,
        _request: tonic::Request<Chat>,
    ) -> Result<tonic::Response<RpcResult>, tonic::Status> {
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
