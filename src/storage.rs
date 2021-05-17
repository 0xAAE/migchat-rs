use super::{Chat, ChatId, InternalError, Post, User, UserId};
use bytes::BytesMut;
use log::{debug, error};
use prost::Message;
use std::path::Path;

const BUCKET_USERS: &str = "users";
const BUCKET_CHATS: &str = "chats";
const BUCKET_POSTS: &str = "posts";

pub struct Storage {
    db: jammdb::DB,
}

impl Storage {
    pub fn new<P: AsRef<Path>>(db_file: P) -> Result<Self, InternalError> {
        let db = jammdb::DB::open(db_file)?;
        // create users bucket in DB if not exists
        let tx = db.tx(true)?;
        match tx.create_bucket(BUCKET_USERS) {
            Ok(_) => {}
            Err(jammdb::Error::BucketExists) => {}
            Err(e) => return Err(format!("{}", e).into()),
        }
        tx.commit()?;
        // create chats bucket in DB if not exists
        let tx = db.tx(true)?;
        match tx.create_bucket(BUCKET_CHATS) {
            Ok(_) => {}
            Err(jammdb::Error::BucketExists) => {}
            Err(e) => return Err(format!("{}", e).into()),
        }
        tx.commit()?;
        // create posts bucket in DB if not exists
        let tx = db.tx(true)?;
        match tx.create_bucket(BUCKET_POSTS) {
            Ok(_) => {}
            Err(jammdb::Error::BucketExists) => {}
            Err(e) => return Err(format!("{}", e).into()),
        }
        tx.commit()?;
        Ok(Self { db })
    }

    // operations with users

    pub fn read_user(&self, id: UserId) -> Result<Option<User>, InternalError> {
        self.read_from_db::<User>(BUCKET_USERS, &id.to_le_bytes())
    }

    pub fn write_user(&self, id: UserId, user: &User) -> Result<(), InternalError> {
        self.write_to_db::<User>(BUCKET_USERS, &id.to_le_bytes(), user)
    }

    /// Tries to conditionally update specified user.
    /// Returns:
    /// - InternalError if some error happens
    /// - Ok(Some(user)) if user was found and successfully updated; user contains *new* value
    /// - Ok(Some(user)) if user was found but updater returned false; user contains *unchanged* value, and database remains unchanged
    /// - Ok(None) if user was not found
    pub fn _update_user<F: FnMut(&mut User) -> bool>(
        &self,
        id: UserId,
        updater: F,
    ) -> Result<Option<User>, InternalError> {
        self.update_in_db::<User, _>(BUCKET_USERS, &id.to_le_bytes(), updater)
    }

    pub fn read_all_users(&self) -> Result<Vec<User>, InternalError> {
        self.read_all_from_db::<User>(BUCKET_USERS)
    }

    #[allow(dead_code)]
    pub fn remove_user(&self, id: UserId) -> Result<(), InternalError> {
        // remove the user out of all chats
        self.update_all_in_db::<Chat, _>(BUCKET_CHATS, |mut_ref_chat| {
            let cnt_before = mut_ref_chat.users.len();
            mut_ref_chat.users.retain(|&u| u != id);
            mut_ref_chat.users.len() < cnt_before
        })
        .and(self.remove_from_db::<User>(BUCKET_USERS, &id.to_le_bytes()))
    }

    // operations with chats

    pub fn read_chat(&self, id: ChatId) -> Result<Option<Chat>, InternalError> {
        self.read_from_db::<Chat>(BUCKET_CHATS, &id.to_le_bytes())
    }

    pub fn write_chat(&self, id: ChatId, chat: &Chat) -> Result<(), InternalError> {
        self.write_to_db::<Chat>(BUCKET_CHATS, &id.to_le_bytes(), chat)
    }

    /// Tries to conditionally update specified chat.
    /// Returns:
    /// - InternalError if some error happens
    /// - Ok(Some(chat)) if chat was found and successfully updated; chat contains *new* value
    /// - Ok(Some(chat)) if chat was found but updater returned false; chat contains *unchanged* value, and database remains unchanged
    /// - Ok(None) if chat was not found
    pub fn update_chat<F: FnMut(&mut Chat) -> bool>(
        &self,
        id: ChatId,
        updater: F,
    ) -> Result<Option<Chat>, InternalError> {
        self.update_in_db::<Chat, _>(BUCKET_CHATS, &id.to_le_bytes(), updater)
    }

    pub fn read_all_chats(&self) -> Result<Vec<Chat>, InternalError> {
        self.read_all_from_db::<Chat>(BUCKET_CHATS)
    }

    pub fn remove_chat(&self, id: ChatId) -> Result<(), InternalError> {
        self.remove_chat_posts(id)
            .and(self.remove_from_db::<Chat>(BUCKET_CHATS, &id.to_le_bytes()))
    }

    // generic operations with user / chats implementation

    fn read_from_db<M: Message + Default>(
        &self,
        bucket_name: &str,
        id: &[u8],
    ) -> Result<Option<M>, InternalError> {
        match self.db.tx(false) {
            Ok(tx) => match tx.get_bucket(bucket_name) {
                Ok(bucket) => match bucket.get(id) {
                    None => Ok(None),
                    Some(record) => {
                        let bin = record.kv().value();
                        match M::decode(bin) {
                            Ok(item) => Ok(Some(item)),
                            Err(e) => {
                                error!("protobuf parse, {}", e);
                                Err(format!("{}", e).into())
                            }
                        }
                    }
                },
                Err(e) => Err(format!("{}", e).into()),
            },
            Err(e) => Err(format!("{}", e).into()),
        }
    }

    fn write_to_db<M: Message>(
        &self,
        bucket_name: &str,
        id: &[u8],
        item: &M,
    ) -> Result<(), InternalError> {
        match self.db.tx(true) {
            Ok(tx) => match tx.get_bucket(bucket_name) {
                Ok(bucket) => {
                    let mut buf = BytesMut::new();
                    match item.encode(&mut buf) {
                        Ok(_) => match bucket.put(id, buf) {
                            Ok(_) => tx.commit().map_err(|e| e.into()),
                            Err(e) => Err(e.into()),
                        },
                        Err(e) => Err(e.into()),
                    }
                }
                Err(e) => Err(e.into()),
            },
            Err(e) => Err(e.into()),
        }
    }

    /// Tries to conditionally update specified item.
    /// Returns:
    /// - InternalError if some error happens
    /// - Ok(Some(item)) if item was found and successfully updated; item contains *new* value
    /// - Ok(Some(item)) if item was found but updater returned false; item contains *unchanged* value, and database remains unchanged
    /// - Ok(None) if item was not found
    fn update_in_db<M: Message + Default + Clone, F: FnMut(&mut M) -> bool>(
        &self,
        bucket_name: &str,
        id: &[u8],
        mut updater: F,
    ) -> Result<Option<M>, InternalError> {
        match self.db.tx(true) {
            Ok(tx) => match tx.get_bucket(bucket_name) {
                Ok(bucket) => {
                    if let Some(kv) = bucket.get_kv(id) {
                        // read
                        let bin = kv.value();
                        match M::decode(bin) {
                            Ok(mut item) => {
                                // update
                                if !updater(&mut item) {
                                    return Ok(Some(item));
                                }
                                // serialize
                                let mut buf = BytesMut::new();
                                match item.encode(&mut buf) {
                                    Ok(_) => match bucket.put(id, buf) {
                                        // store into db
                                        Ok(_) => {
                                            tx.commit().map(|_| Some(item)).map_err(|e| e.into())
                                        }
                                        Err(e) => Err(e.into()),
                                    },
                                    Err(e) => Err(e.into()),
                                }
                            }
                            Err(e) => {
                                error!("protobuf parse, {}", e);
                                Err(e.into())
                            }
                        }
                    } else {
                        Ok(None)
                    }
                }
                Err(e) => Err(e.into()),
            },
            Err(e) => Err(e.into()),
        }
    }

    /// Tries to conditionally update all items in specified bucket.
    /// Returns:
    /// - InternalError if some error happens
    /// - Ok(count) if some items were updated
    /// - Ok(0) if no item was updated
    fn update_all_in_db<M: Message + Default + Clone, F: FnMut(&mut M) -> bool>(
        &self,
        bucket_name: &str,
        mut updater: F,
    ) -> Result<usize, InternalError> {
        match self.db.tx(true) {
            Ok(tx) => match tx.get_bucket(bucket_name) {
                Ok(bucket) => {
                    let mut cnt_updated = 0;
                    for pair in bucket.kv_pairs() {
                        // read
                        let bin = pair.value();
                        match M::decode(bin) {
                            Ok(mut item) => {
                                if updater(&mut item) {
                                    let mut buf = BytesMut::new();
                                    if item.encode(&mut buf).is_ok() {
                                        match bucket.put(pair.key(), buf) {
                                            Ok(_) => cnt_updated += 1,
                                            Err(e) => error!("failed to store updated item, {}", e),
                                        }
                                    }
                                }
                            }
                            Err(e) => error!("failed to parse item, {}", e),
                        }
                    }
                    if cnt_updated > 0 {
                        tx.commit().map(|_| cnt_updated).map_err(|e| e.into())
                    } else {
                        Ok(0)
                    }
                }
                Err(e) => Err(e.into()),
            },
            Err(e) => Err(e.into()),
        }
    }

    fn read_all_from_db<M: Message + Default>(
        &self,
        bucket_name: &str,
    ) -> Result<Vec<M>, InternalError> {
        match self.db.tx(false) {
            Ok(tx) => match tx.get_bucket(bucket_name) {
                Ok(bucket) => {
                    let mut items = Vec::new();
                    for pair in bucket.kv_pairs() {
                        let bin = pair.value();
                        match M::decode(bin) {
                            Ok(user) => items.push(user),
                            Err(e) => error!("internal error, {}", e),
                        }
                    }
                    Ok(items)
                }
                Err(e) => Err(e.into()),
            },
            Err(e) => Err(e.into()),
        }
    }

    fn remove_from_db<M: Message>(
        &self,
        bucket_name: &str,
        id: &[u8],
    ) -> Result<(), InternalError> {
        // does not require writable tx!
        // see https://docs.rs/jammdb/0.5.0/jammdb/struct.Bucket.html#method.delete
        match self.db.tx(true) {
            Ok(tx) => match tx.get_bucket(bucket_name) {
                Ok(bucket) => bucket
                    .delete(id)
                    .map(|_| ())
                    .map_err(|e| format!("{}", e).into()),
                Err(e) => Err(e.into()),
            },
            Err(e) => Err(e.into()),
        }
    }

    // operations with posts
    // all posts stored groupped by their chats into separate buckets: BUCKET_POSTS/chat_id/*
    // the post's key in the storage is a sequential integer to preserve posts natural order
    pub fn write_post(&self, post: &Post) -> Result<(), InternalError> {
        match self.db.tx(true) {
            Ok(tx) => match tx.get_or_create_bucket(BUCKET_POSTS) {
                Ok(posts_bucket) => {
                    match posts_bucket.get_or_create_bucket(&post.chat_id.to_le_bytes()) {
                        Ok(chat_bucket) => {
                            let mut buf = BytesMut::new();
                            match post.encode(&mut buf) {
                                Ok(_) => {
                                    let k = chat_bucket.next_int();
                                    match chat_bucket.put(&k.to_le_bytes(), buf) {
                                        Ok(_) => tx.commit().map_err(|e| e.into()),
                                        Err(e) => Err(e.into()),
                                    }
                                }
                                Err(e) => Err(e.into()),
                            }
                        }
                        Err(e) => Err(e.into()),
                    }
                }
                Err(e) => Err(e.into()),
            },
            Err(e) => Err(e.into()),
        }
    }

    pub fn chat_posts_count(&self, chat_id: ChatId) -> Result<usize, InternalError> {
        match self.db.tx(false) {
            Ok(tx) => match tx.get_bucket(BUCKET_POSTS) {
                Ok(posts_bucket) => match posts_bucket.get_bucket(&chat_id.to_le_bytes()) {
                    Ok(chat_bucket) => Ok(chat_bucket.kv_pairs().count()),
                    Err(jammdb::Error::BucketMissing) => Ok(0),
                    Err(e) => Err(e.into()),
                },
                Err(e) => Err(e.into()),
            },
            Err(e) => Err(e.into()),
        }
    }

    pub fn read_chat_posts(
        &self,
        chat_id: ChatId,
        idx_from: usize,
        count: usize,
    ) -> Result<Vec<Post>, InternalError> {
        if count == 0 {
            return Ok(Vec::new());
        }
        match self.db.tx(false) {
            Ok(tx) => match tx.get_bucket(BUCKET_POSTS) {
                Ok(posts_bucket) => match posts_bucket.get_bucket(&chat_id.to_le_bytes()) {
                    Ok(chat_bucket) => {
                        let mut posts = Vec::new();
                        for pair in chat_bucket.kv_pairs().skip(idx_from).take(count) {
                            let bin = pair.value();
                            match Post::decode(bin) {
                                Ok(post) => posts.push(post),
                                Err(e) => error!("internal error, {}", e),
                            }
                        }
                        Ok(posts)
                    }
                    Err(jammdb::Error::BucketMissing) => {
                        debug!("there wasn't any posts in requested chat");
                        Ok(Vec::new())
                    }
                    Err(e) => Err(e.into()),
                },
                Err(e) => Err(e.into()),
            },
            Err(e) => Err(e.into()),
        }
    }

    fn remove_chat_posts(&self, id: ChatId) -> Result<(), InternalError> {
        match self.db.tx(true) {
            Ok(tx) => match tx.get_bucket(BUCKET_POSTS) {
                Ok(posts_bucket) => posts_bucket
                    .delete_bucket(&id.to_le_bytes())
                    .and(tx.commit())
                    .map_err(|e| e.into()),
                Err(e) => Err(e.into()),
            },
            Err(e) => Err(e.into()),
        }
    }
}

#[cfg(test)]
mod tests {

    use super::*;

    const TEST_DB: &str = "migchat-test-storage.db";
    const BUCKET_ROOT: &str = "ROOT";

    #[test]
    fn test_write_post() {
        let _ = std::fs::remove_file(TEST_DB);
        {
            let storage = Storage::new(TEST_DB).unwrap();
            let post = Post {
                id: 1,
                chat_id: 2,
                user_id: 3,
                text: String::from("text"),
                attachments: Vec::new(),
                created: 0,
            };
            let res = storage.write_post(&post);
            assert!(res.is_ok());
        }
        let _ = std::fs::remove_file(TEST_DB);
    }

    #[test]
    fn test_get_or_create_bucket() {
        let _ = std::fs::remove_file(TEST_DB);
        {
            let db = jammdb::DB::open(TEST_DB).unwrap();
            {
                let tx = db.tx(true).unwrap();
                let _bucket = tx.get_or_create_bucket(BUCKET_ROOT).unwrap();
                tx.commit().unwrap();
            }
            {
                let tx = db.tx(true).unwrap();
                let root_bucket = tx.get_bucket(BUCKET_ROOT).unwrap();
                let _posts_bucket = root_bucket.get_or_create_bucket(BUCKET_POSTS).unwrap();
                tx.commit().unwrap();
            }
            {
                let mut post = Post {
                    id: 1000,
                    chat_id: 2000,
                    user_id: 3,
                    text: String::from("text"),
                    attachments: Vec::new(),
                    created: 0,
                };

                match db.tx(true) {
                    Ok(tx) => match tx.get_bucket(BUCKET_ROOT) {
                        Ok(root_bucket) => match root_bucket.get_or_create_bucket(BUCKET_POSTS) {
                            Ok(posts_bucket) => {
                                match posts_bucket.create_bucket(&post.chat_id.to_le_bytes()) {
                                    Ok(chat_bucket) => {
                                        let mut buf = BytesMut::new();
                                        match post.encode(&mut buf) {
                                            Ok(_) => {
                                                assert_eq!(chat_bucket.next_int(), 0);
                                                assert_eq!(chat_bucket.next_int(), 0);
                                                post.id = chat_bucket.next_int();
                                                match chat_bucket.put(&post.id.to_le_bytes(), buf) {
                                                    Ok(_) => {
                                                        assert_eq!(chat_bucket.next_int(), 1);
                                                        assert!(tx.commit().is_ok());
                                                    }
                                                    Err(_e) => assert!(false),
                                                }
                                            }
                                            Err(_e) => assert!(false),
                                        }
                                    }
                                    Err(_e) => assert!(false),
                                }
                            }
                            Err(_e) => assert!(false),
                        },
                        Err(_e) => assert!(false),
                    },
                    Err(_e) => assert!(false),
                };
            }
        }
        let _ = std::fs::remove_file(TEST_DB);
    }
}
