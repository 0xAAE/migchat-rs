use tonic::{transport::Server, Request, Response, Status};

use futures::{Stream, StreamExt};
use migchat::chat_room_service_server::{ChatRoomService, ChatRoomServiceServer};
use migchat::{
    Chat, ChatInfo, Invitation, Post, Registration, RequestChats, RequestUsers,
    Result as RpcResult, UpdateChats, UpdateUsers, UserInfo,
};
use std::pin::Pin;
use tokio::sync::mpsc;
use uuid::Uuid;

pub mod migchat {
    tonic::include_proto!("migchat"); // The string specified here must match the proto package name
}

#[derive(Debug, Default)]
pub struct ChatRoomImpl {}

#[tonic::async_trait]
impl ChatRoomService for ChatRoomImpl {
    #[doc = " Sends a reqistration request"]
    async fn register(&self, request: Request<UserInfo>) -> Result<Response<Registration>, Status> {
        println!("register: {:?}", request);
        let uuid = migchat::Uuid {
            value: format!("{}", Uuid::new_v4()),
        };
        let reply = Registration { uuid: Some(uuid) };
        Ok(Response::new(reply)) // Send back our formatted greeting
    }

    #[doc = "Server streaming response type for the Login method."]
    type LoginStream =
        Pin<Box<dyn Stream<Item = Result<Invitation, tonic::Status>> + Send + Sync + 'static>>;

    #[doc = " Sends a login"]
    async fn login(
        &self,
        request: tonic::Request<Registration>,
    ) -> Result<tonic::Response<Self::LoginStream>, tonic::Status> {
        // create stream of invitations, the first is from server to logged user
        println!("login: {:?}", request);

        let (tx, rx) = mpsc::channel(4);

        tokio::spawn(async move {
            let invitation = Invitation {
                chat_id: 0,
                session_id: 1,
            };
            tx.send(Ok(invitation)).await.unwrap();
        });

        Ok(Response::new(Box::pin(
            tokio_stream::wrappers::ReceiverStream::new(rx),
        )))
    }

    #[doc = " Sends a logout request using the first invitation from the server"]
    async fn logout(
        &self,
        request: tonic::Request<Invitation>,
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
        request: tonic::Request<tonic::Streaming<Post>>,
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
        request: tonic::Request<RequestUsers>,
    ) -> Result<tonic::Response<Self::GetUsersStream>, tonic::Status> {
        Err(tonic::Status::unimplemented(
            "get_users() is not implemented yet",
        ))
    }

    #[doc = "Server streaming response type for the GetChats method."]
    type GetChatsStream =
        Pin<Box<dyn Stream<Item = Result<UpdateChats, tonic::Status>> + Send + Sync + 'static>>;

    #[doc = " Asks for chats list"]
    async fn get_chats(
        &self,
        request: tonic::Request<RequestChats>,
    ) -> Result<tonic::Response<Self::GetChatsStream>, tonic::Status> {
        Err(tonic::Status::unimplemented(
            "get_chats() is not implemented yet",
        ))
    }

    #[doc = " Creates new chat"]
    async fn create_chat(
        &self,
        request: tonic::Request<ChatInfo>,
    ) -> Result<tonic::Response<Chat>, tonic::Status> {
        Err(tonic::Status::unimplemented(
            "create_chat() is not implemented yet",
        ))
    }

    #[doc = " Invites user to chat"]
    async fn invite_user(
        &self,
        request: tonic::Request<Invitation>,
    ) -> Result<tonic::Response<RpcResult>, tonic::Status> {
        Err(tonic::Status::unimplemented(
            "invite_user() is not implemented yet",
        ))
    }

    #[doc = " Enters the chat"]
    async fn enter_chat(
        &self,
        request: tonic::Request<Chat>,
    ) -> Result<tonic::Response<RpcResult>, tonic::Status> {
        Err(tonic::Status::unimplemented(
            "enter_chat() is not implemented yet",
        ))
    }

    #[doc = " Leaves active chat"]
    async fn leave_chat(
        &self,
        request: tonic::Request<Chat>,
    ) -> Result<tonic::Response<RpcResult>, tonic::Status> {
        Err(tonic::Status::unimplemented(
            "leave_chat() is not implemented yet",
        ))
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let addr = "0.0.0.0:50051".parse().unwrap();
    let chat_room = ChatRoomImpl::default();
    println!("Chat room is listening on {}", addr);

    let svc = ChatRoomServiceServer::new(chat_room);
    Server::builder().add_service(svc).serve(addr).await?;

    Ok(())
}
