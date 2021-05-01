use chrono::prelude::*;
use futures::Stream; //, StreamExt};
use fxhash::FxHasher64;
use log::{debug, error};
use std::{collections::BTreeSet, hash::Hasher, ops::Deref, pin::Pin, sync::Arc};
use tokio::sync::mpsc;
use tonic::{Request, Response, Status};

use super::proto::chat_room_service_server::ChatRoomService;
use super::proto::{
    ChatInfo, ChatReference, Invitation, Post, Registration, RegistrationInfo, Result as RpcResult,
    UpdateChats, UpdateUsers, UserInfo, NOT_CHAT_ID, NOT_POST_ID,
};
use super::{Chat, ChatChanged, ChatRoomImpl, User, UserChanged, UserId};

fn get_user_id(user: &UserInfo) -> u64 {
    let mut hasher = FxHasher64::default();
    hasher.write(user.name.as_bytes());
    hasher.write(user.short_name.as_bytes());
    hasher.finish()
}

fn new_post_id() -> u64 {
    let mut v = NOT_POST_ID;
    while v == NOT_POST_ID {
        v = rand::random();
    }
    v
}

fn new_chat_id() -> u64 {
    let mut v = NOT_CHAT_ID;
    while v == NOT_CHAT_ID {
        v = rand::random();
    }
    v
}

// chat must have a predictable reproducable id
// based on its description if is not empty or its members,
// the reasons are
// - avoid having chats with empty names in chat list
// - display such a chat like a dialog of its members
// - chat must be discoverable by any member instead of creating new and new ones
fn get_chat_id(description: &str, users: &[UserId]) -> u64 {
    let mut hasher = FxHasher64::default();
    if !description.is_empty() {
        hasher.write(description.as_bytes());
        hasher.finish()
    } else if !users.is_empty() {
        for id in users {
            hasher.write(&id.to_le_bytes());
        }
        hasher.finish()
    } else {
        new_chat_id()
    }
}

// return false if chat is invisible for specified user
fn is_chat_visible_for(chat: &Chat, user_id: UserId) -> bool {
    if chat.description.is_empty() {
        chat.users.contains(&user_id)
    } else {
        true
    }
}

#[tonic::async_trait]
impl ChatRoomService for ChatRoomImpl {
    #[doc = " Sends a reqistration request"]
    async fn register(
        &self,
        request: Request<UserInfo>,
    ) -> Result<Response<RegistrationInfo>, Status> {
        debug!("register(): {:?}", &request);
        let user_info = request.into_inner();
        let id = get_user_id(&user_info);
        if let Ok(mut online_users) = self.online_users.write() {
            online_users.insert(id);
        } else {
            error!("fatal internal, failed to access online users collection");
        }
        // test existing
        match self.storage.read_user(id) {
            Err(e) => return Err(tonic::Status::internal(format!("{}", e))),
            Ok(opt) => {
                if let Some(u) = opt {
                    debug!("{} ({}) already registered", u.short_name, u.name);
                    if !self.notify_user_changed(UserChanged::Online(u.id)).await {
                        self.actualize_user_listeners();
                    }
                    return Ok(Response::new(RegistrationInfo {
                        registration: Some(Registration { user_id: id }),
                        created: u.created,
                    }));
                }
            }
        }
        let new_user = User {
            id,
            name: user_info.name,
            short_name: user_info.short_name,
            created: Utc::now().timestamp() as u64,
        };
        // store new user
        if !self
            .notify_user_changed(UserChanged::Info(Arc::new(new_user.clone())))
            .await
        {
            self.actualize_user_listeners();
        }
        if let Err(e) = self.storage.write_user(new_user.id, &new_user) {
            Err(tonic::Status::internal(format!("{}", e)))
        } else {
            Ok(Response::new(RegistrationInfo {
                registration: Some(Registration { user_id: id }),
                created: new_user.created,
            }))
        }
    }

    #[doc = "Server streaming response type for the GetInvitations method."]
    type GetInvitationsStream =
        Pin<Box<dyn Stream<Item = Result<Invitation, tonic::Status>> + Send + Sync + 'static>>;

    #[doc = " Asks for invitations"]
    async fn get_invitations(
        &self,
        request: tonic::Request<Registration>,
    ) -> Result<tonic::Response<Self::GetInvitationsStream>, tonic::Status> {
        // get source channel of invitations
        debug!("get_invitations(): {:?}", &request);
        let user_id = request.into_inner().user_id;
        let (listener, notifier) = mpsc::channel(4);
        if let Ok(mut listeners) = self.invitations_listeners.write() {
            // test alive
            listeners.retain(|k, v| {
                if v.is_closed() {
                    debug!("stop streaming invitations to {}", k);
                    false
                } else {
                    true
                }
            });
            // add new
            listeners.insert(user_id, listener);
        } else {
            return Err(tonic::Status::internal("no access to invitation channel"));
        };
        // launch stream source
        let (tx, rx) = mpsc::channel(4);
        tokio::spawn(async move {
            debug!("start streaming invitations to {}", user_id);
            let mut notifier = notifier;
            while let Some(invitation) = notifier.recv().await {
                if let Err(e) = tx.send(Ok(invitation)).await {
                    error!("failed streaming invoitations: {}", e);
                    break;
                }
            }
            debug!("stream of invitations to {} has stopped", user_id);
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
            if listeners.remove(&user_id).is_some() {
                debug!("stop streaming users to {}", user_id);
            }
        } else {
            error!("failed locking users listeners (logout)");
        }
        if let Ok(mut listeners) = self.chats_listeners.write() {
            if listeners.remove(&user_id).is_some() {
                debug!("stop streaming chats to {}", user_id);
            }
        } else {
            error!("failed locking chats listeners (logout)");
        }
        if let Ok(mut listeners) = self.invitations_listeners.write() {
            if listeners.remove(&user_id).is_some() {
                debug!("stop streaming invitations to {}", user_id);
            }
        } else {
            error!("failed locking invitations listeners (logout)");
        }
        if let Ok(mut listeners) = self.posts_listeners.write() {
            if listeners.remove(&user_id).is_some() {
                debug!("stop streaming posts to {}", user_id);
            }
        } else {
            error!("failed locking posts listeners (logout)");
        }
        if let Ok(mut online_users) = self.online_users.write() {
            online_users.remove(&user_id);
        }
        if !self
            .notify_user_changed(UserChanged::Offline(user_id))
            .await
        {
            self.actualize_user_listeners();
        }
        Ok(Response::new(RpcResult {
            ok: true,
            description: String::from("logout successful"),
        }))
    }

    #[doc = "Server streaming response type for the GetPosts method."]
    type GetPostsStream =
        Pin<Box<dyn Stream<Item = Result<Post, tonic::Status>> + Send + Sync + 'static>>;

    #[doc = " Asks for incoming posts"]
    async fn get_posts(
        &self,
        request: tonic::Request<Registration>,
    ) -> Result<tonic::Response<Self::GetPostsStream>, tonic::Status> {
        debug!("get_posts(): {:?}", &request);
        let user_id = request.into_inner().user_id;
        let (listener, notifier) = mpsc::channel::<Arc<Post>>(4);
        if let Ok(mut listeners) = self.posts_listeners.write() {
            listeners.insert(user_id, listener);
        } else {
            return Err(tonic::Status::internal("no access to posts listeners"));
        }
        // collect existing posts from chats where user is a member
        let mut existing = Vec::new();
        if let Ok(chats) = self.storage.read_all_chats() {
            for chat in chats {
                if chat.users.contains(&user_id) {
                    match self.storage.read_chat_posts(chat.id) {
                        Ok(mut posts) => existing.append(&mut posts),
                        Err(e) => error!("failed to read posts, {}", e),
                    }
                }
            }
        } else {
            error!("failed to read chats");
        }
        // start permanent listener that streams data to remote client
        let (tx, rx) = mpsc::channel(4);
        tokio::spawn(async move {
            debug!("start streaming posts to {}", user_id);
            if !existing.is_empty() {
                // send existing posts
                for post in existing {
                    if let Err(e) = tx.send(Ok(post)).await {
                        error!("failed sending existing post: {}", e);
                    }
                }
            }
            // re-translate new users
            let mut notifier = notifier;
            while let Some(post) = notifier.recv().await {
                debug!("re-translating new post to {}", user_id);
                if let Err(e) = tx.send(Ok((*post).clone())).await {
                    error!("failed sending post: {}, stop", e);
                    break;
                }
            }
            debug!("stream of posts to {} has stopped", user_id);
        });
        // start streaming activity, data consumer
        Ok(Response::new(Box::pin(
            tokio_stream::wrappers::ReceiverStream::new(rx),
        )))
    }

    #[doc = "Server streaming response type for the GetUsers method."]
    type GetUsersStream =
        Pin<Box<dyn Stream<Item = Result<UpdateUsers, tonic::Status>> + Send + Sync + 'static>>;

    #[doc = " Asks for contacts list"]
    async fn get_users(
        &self,
        request: tonic::Request<Registration>,
    ) -> Result<tonic::Response<Self::GetUsersStream>, tonic::Status> {
        debug!("get_users(): {:?}", &request);
        let user_id = request.into_inner().user_id;
        let (listener, notifier) = mpsc::channel::<UserChanged>(4);
        if let Ok(mut listeners) = self.users_listeners.write() {
            // test alive
            listeners.retain(|k, v| {
                if v.is_closed() {
                    debug!("stop streaming users to {}", k);
                    false
                } else {
                    true
                }
            });
            // add new
            listeners.insert(user_id, listener);
        } else {
            return Err(tonic::Status::internal("no access to users listeners"));
        }
        // collect existing users
        let existing = if let Ok(mut users) = self.storage.read_all_users() {
            users.retain(|u| u.id != user_id);
            users
        } else {
            error!("failed to read existing users");
            Vec::new()
        };
        // collect statuses
        let mut online = Vec::new();
        let mut offline = Vec::new();
        if let Ok(online_users) = self.online_users.read() {
            for u in &existing {
                if online_users.contains(&u.id) {
                    online.push(u.id);
                } else {
                    offline.push(u.id);
                }
            }
        } else {
            error!("fatal internal, failed to access users statuses");
        }
        // start permanent listener that streams data to remote client
        let (tx, rx) = mpsc::channel(4);
        tokio::spawn(async move {
            debug!("start streaming users to {}", user_id);
            if !existing.is_empty() {
                // send existing users
                let start_update = UpdateUsers {
                    added: existing,
                    online,
                    offline,
                };
                debug!(
                    "sending {} existing users to {}",
                    start_update.added.len(),
                    user_id
                );
                if let Err(e) = tx.send(Ok(start_update)).await {
                    error!("failed sending existing users: {}", e);
                }
            }
            // re-translate new users, all new users will start with online status
            let mut notifier = notifier;
            while let Some(notification) = notifier.recv().await {
                let update = match notification {
                    UserChanged::Info(user) => {
                        debug!("re-translating new online user {} to {}", user.id, user_id);
                        UpdateUsers {
                            added: vec![user.deref().clone()],
                            online: vec![user.id],
                            offline: Vec::new(),
                        }
                    }
                    UserChanged::Online(id) => {
                        debug!("re-translating entered {} to {}", id, user_id);
                        UpdateUsers {
                            added: Vec::new(),
                            online: vec![user_id],
                            offline: Vec::new(),
                        }
                    }
                    UserChanged::Offline(id) => {
                        debug!("re-translating gone {} to {}", id, user_id);
                        UpdateUsers {
                            added: Vec::new(),
                            online: Vec::new(),
                            offline: vec![user_id],
                        }
                    }
                };
                if let Err(e) = tx.send(Ok(update)).await {
                    error!("failed sending users update: {}, stop", e);
                    break;
                }
            }
            debug!("stream of users to {} has stopped", user_id);
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
        request: tonic::Request<Registration>,
    ) -> Result<tonic::Response<Self::GetChatsStream>, tonic::Status> {
        debug!("get_chats(): {:?}", &request);
        let user_id = request.into_inner().user_id;
        let (listener, notifier) = mpsc::channel::<ChatChanged>(4);
        if let Ok(mut listeners) = self.chats_listeners.write() {
            listeners.insert(user_id, listener);
        } else {
            // failed locking listeners
            return Err(tonic::Status::internal("no access to chat listeners"));
        }
        // collect existing chats
        let existing = if let Ok(mut chats) = self.storage.read_all_chats() {
            chats.retain(|c| is_chat_visible_for(&c, user_id));
            chats
        } else {
            error!("failed to read existing chats");
            Vec::new()
        };
        // start permanent listener that streams data to remote client
        let (tx, rx) = mpsc::channel(4);
        tokio::spawn(async move {
            debug!("start streaming chats to {}", user_id);
            if !existing.is_empty() {
                // send existing chats
                let start_update = UpdateChats {
                    updated: existing,
                    gone: Vec::new(),
                };
                debug!(
                    "sending {} existing chats to {}",
                    start_update.updated.len(),
                    user_id
                );
                if let Err(e) = tx.send(Ok(start_update)).await {
                    error!("failed sending existing chats: {}", e);
                }
            }
            // re-translate new chats
            let mut notifier = notifier;
            while let Some(notification) = notifier.recv().await {
                let update = match notification {
                    ChatChanged::Updated(chat) => {
                        if !is_chat_visible_for(&chat, user_id) {
                            continue;
                        }
                        debug!("re-translating new chat to {}", user_id);
                        UpdateChats {
                            updated: vec![(*chat).clone()],
                            gone: Vec::new(),
                        }
                    }
                    ChatChanged::Closed(id) => {
                        debug!("re-translating closed chat to {}", user_id);
                        UpdateChats {
                            updated: Vec::new(),
                            gone: vec![id],
                        }
                    }
                };
                if let Err(e) = tx.send(Ok(update)).await {
                    error!("failed sending chat: {}", e);
                    break;
                }
            }
            debug!("stop streaming chats to {}", user_id);
        });
        // start streaming activity, data consumer
        Ok(Response::new(Box::pin(
            tokio_stream::wrappers::ReceiverStream::new(rx),
        )))
    }

    #[doc = " Creates new post"]
    async fn create_post(
        &self,
        request: tonic::Request<Post>,
    ) -> Result<tonic::Response<RpcResult>, tonic::Status> {
        debug!("create_post(): {:?}", &request);
        let mut post = request.into_inner();
        if post.id != NOT_POST_ID {
            return Err(tonic::Status::invalid_argument(format!(
                "id must be {}",
                NOT_POST_ID
            )));
        }
        post.id = new_post_id();
        post.created = Utc::now().timestamp() as u64;
        if let Err(e) = self.storage.write_post(&post) {
            error!("failed to save post, {}", e);
        }
        if !self.notify_new_post(post).await {
            self.actualize_post_listeners();
        }
        Ok(Response::new(RpcResult {
            ok: true,
            description: String::from("accepted"),
        }))
    }

    #[doc = " Creates new chat"]
    async fn create_chat(
        &self,
        request: tonic::Request<ChatInfo>,
    ) -> Result<tonic::Response<Chat>, tonic::Status> {
        debug!("create_chat(): {:?}", &request);
        let info = request.get_ref();
        let users = if info.auto_enter {
            // filter out duplicated users and sort them as well
            let mut tmp = BTreeSet::new();
            tmp.insert(info.user_id);
            for u in &info.desired_users {
                tmp.insert(*u);
            }
            tmp.into_iter().collect()
        } else {
            Vec::new()
        };
        let id = get_chat_id(&info.description, &users);
        // test chat exists and enter the chat if that has not been done before
        match self.storage.update_chat(id, |mut_ref_chat| {
            if info.auto_enter && !mut_ref_chat.users.contains(&info.user_id) {
                mut_ref_chat.users.push(info.user_id);
                true
            } else {
                false
            }
        }) {
            Ok(Some(chat)) => {
                // chat was found & updated if needed
                if !self
                    .notify_chat_changed(ChatChanged::Updated(Arc::new(chat.clone())))
                    .await
                {
                    self.actualize_chat_listeners();
                }
                Ok(Response::new(chat))
            }
            Ok(None) => {
                // chat was not found, add new
                let chat = Chat {
                    id,
                    permanent: info.permanent,
                    description: info.description.clone(),
                    users,
                    created: Utc::now().timestamp() as u64,
                };
                if let Err(e) = self.storage.write_chat(id, &chat) {
                    Err(tonic::Status::internal(format!(
                        "failed to create chat, {}",
                        e
                    )))
                } else {
                    if !self
                        .notify_chat_changed(ChatChanged::Updated(Arc::new(chat.clone())))
                        .await
                    {
                        self.actualize_chat_listeners();
                    }
                    Ok(Response::new(chat))
                }
            }
            Err(e) => Err(tonic::Status::internal(format!(
                "failed to access chats, {}",
                e
            ))),
        }
    }

    #[doc = " Invites user to chat"]
    async fn invite_user(
        &self,
        request: tonic::Request<Invitation>,
    ) -> Result<tonic::Response<RpcResult>, tonic::Status> {
        debug!("invite_user(): {:?}", &request);
        let invitation = request.into_inner();
        // test chat exists
        match self.storage.read_chat(invitation.chat_id) {
            Err(e) => return Err(tonic::Status::internal(format!("failed read chats, {}", e))),
            Ok(None) => {
                return Err(tonic::Status::not_found(format!(
                    "chat {} does not exist",
                    invitation.chat_id
                )))
            }
            _ => {}
        }
        // test recepient exists
        match self.storage.read_user(invitation.to_user_id) {
            Err(e) => return Err(tonic::Status::internal(format!("{}", e))),
            Ok(opt) => {
                if opt.is_none() {
                    return Err(tonic::Status::not_found(format!(
                        "user {} is not registered",
                        invitation.to_user_id
                    )));
                }
            }
        }
        // try to get send channel and send invitation
        let tx = if let Ok(listeners) = self.invitations_listeners.read() {
            if let Some(tx) = listeners.get(&invitation.to_user_id) {
                tx.clone()
            } else {
                return Err(tonic::Status::not_found(format!(
                    "{} did not subscribe to invitations",
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
        request: tonic::Request<ChatReference>,
    ) -> Result<tonic::Response<RpcResult>, tonic::Status> {
        debug!("enter_chat(): {:?}", &request);
        let chat_ref = request.into_inner();
        match self.storage.update_chat(chat_ref.chat_id, |mut_ref_chat| {
            if !mut_ref_chat.users.contains(&chat_ref.user_id) {
                mut_ref_chat.users.push(chat_ref.user_id);
                true
            } else {
                false
            }
        }) {
            Ok(Some(chat)) => {
                if !self
                    .notify_chat_changed(ChatChanged::Updated(Arc::new(chat)))
                    .await
                {
                    self.actualize_chat_listeners();
                }
                Ok(Response::new(RpcResult {
                    ok: true,
                    description: String::from("entered the chat"),
                }))
            }
            Ok(None) => Err(tonic::Status::not_found("chat does not exist")),
            Err(e) => Err(tonic::Status::internal(format!(
                "failed access chats, {}",
                e
            ))),
        }
    }

    #[doc = " Leaves active chat"]
    async fn leave_chat(
        &self,
        request: tonic::Request<ChatReference>,
    ) -> Result<tonic::Response<RpcResult>, tonic::Status> {
        debug!("leave_chat(): {:?}", &request);
        let chat_ref = request.into_inner();
        let updated_chat = match self.storage.update_chat(chat_ref.chat_id, |mut_ref_chat| {
            if mut_ref_chat.users.contains(&chat_ref.user_id) {
                mut_ref_chat.users.retain(|&id| id != chat_ref.user_id);
                true
            } else {
                false
            }
        }) {
            Ok(Some(chat)) => {
                if !self
                    .notify_chat_changed(ChatChanged::Updated(Arc::new(chat.clone())))
                    .await
                {
                    self.actualize_chat_listeners();
                }
                chat
            }
            Ok(None) => return Err(tonic::Status::not_found("chat does not exist")),
            Err(e) => {
                return Err(tonic::Status::internal(format!(
                    "failed access chats, {}",
                    e
                )))
            }
        };
        let all_notified = if !updated_chat.permanent && updated_chat.users.is_empty() {
            //remove chat
            if let Err(e) = self.storage.remove_chat(chat_ref.chat_id) {
                error!("internal, {}", e);
            }
            self.notify_chat_changed(ChatChanged::Closed(chat_ref.chat_id))
                .await
        } else {
            self.notify_chat_changed(ChatChanged::Updated(Arc::new(updated_chat)))
                .await
        };
        if !all_notified {
            self.actualize_chat_listeners();
        }
        Ok(Response::new(RpcResult {
            ok: true,
            description: String::from("entered the chat"),
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sorted_users() {
        let mut collection = BTreeSet::new();
        collection.insert(10);
        collection.insert(2);
        collection.insert(30);
        collection.insert(30);
        collection.insert(30);
        collection.insert(30);
        collection.insert(30);
        collection.insert(30);
        collection.insert(4);
        collection.insert(4);
        collection.insert(4);
        collection.insert(4);
        collection.insert(4);
        collection.insert(40);
        collection.insert(4);
        collection.insert(30);
        assert_eq!(collection.len(), 5);
        assert_eq!(
            collection.into_iter().collect::<Vec<u64>>(),
            vec![2, 4, 10, 30, 40]
        );
    }

    #[test]
    fn chat_identification() {
        let user1 = UserInfo {
            name: "user 1".to_string(),
            short_name: "u1".to_string(),
        };
        let user2 = UserInfo {
            name: "user 2".to_string(),
            short_name: "u2".to_string(),
        };
        let user3 = UserInfo {
            name: "user 3".to_string(),
            short_name: "u3".to_string(),
        };
        let id_u1 = get_user_id(&user1);
        let id_u2 = get_user_id(&user2);
        let id_u3 = get_user_id(&user3);

        let id_c12 = get_chat_id("", &vec![id_u1, id_u2]);
        let id_c13 = get_chat_id("", &vec![id_u1, id_u3]);
        let id_c23 = get_chat_id("", &vec![id_u2, id_u3]);
        let id_c123 = get_chat_id("", &vec![id_u1, id_u2, id_u3]);
        assert_ne!(id_c12, id_c13);
        assert_ne!(id_c12, id_c23);
        assert_ne!(id_c12, id_c123);
        assert_ne!(id_c23, id_c13);
        assert_ne!(id_c123, id_c13);
        assert_ne!(id_c123, id_c23);
    }
}
