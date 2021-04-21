use super::{Chat, ChatId, InternalError, Post, User, UserId};
use bytes::BytesMut;
use log::error;
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

    pub fn _update_user<F: FnMut(&mut User)>(
        &self,
        id: UserId,
        updater: F,
    ) -> Result<Option<User>, InternalError> {
        self.update_in_db::<User, _>(BUCKET_USERS, &id.to_le_bytes(), updater)
    }

    pub fn read_all_users(&self) -> Result<Vec<User>, InternalError> {
        self.read_all_from_db::<User>(BUCKET_USERS)
    }

    pub fn _remove_user(&self, id: UserId) -> Result<(), InternalError> {
        self.remove_from_db::<User>(BUCKET_USERS, &id.to_le_bytes())
    }

    // operations with chats

    pub fn read_chat(&self, id: ChatId) -> Result<Option<Chat>, InternalError> {
        self.read_from_db::<Chat>(BUCKET_CHATS, &id.to_le_bytes())
    }

    pub fn write_chat(&self, id: ChatId, chat: &Chat) -> Result<(), InternalError> {
        self.write_to_db::<Chat>(BUCKET_CHATS, &id.to_le_bytes(), chat)
    }

    pub fn update_chat<F: FnMut(&mut Chat)>(
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
        self.remove_from_db::<Chat>(BUCKET_CHATS, &id.to_le_bytes())
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

    fn update_in_db<M: Message + Default + Clone, F: FnMut(&mut M)>(
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
                            Ok(item) => {
                                let mut new_item = item;
                                // update
                                updater(&mut new_item);
                                // serialize
                                let mut buf = BytesMut::new();
                                match new_item.encode(&mut buf) {
                                    Ok(_) => match bucket.put(id, buf) {
                                        // store into db
                                        Ok(_) => tx
                                            .commit()
                                            .map(|_| Some(new_item))
                                            .map_err(|e| e.into()),
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
        match self.db.tx(false) {
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
            Ok(tx) => match tx.get_bucket(BUCKET_POSTS) {
                Ok(root_bucket) => {
                    match root_bucket.get_or_create_bucket(&post.chat_id.to_le_bytes()) {
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

    pub fn read_chat_posts(&self, chat_id: ChatId) -> Result<Vec<Post>, InternalError> {
        match self.db.tx(false) {
            Ok(tx) => match tx.get_bucket(BUCKET_POSTS) {
                Ok(root_bucket) => match root_bucket.get_bucket(&chat_id.to_le_bytes()) {
                    Ok(chat_bucket) => {
                        let mut posts = Vec::new();
                        for pair in chat_bucket.kv_pairs() {
                            let bin = pair.value();
                            match Post::decode(bin) {
                                Ok(post) => posts.push(post),
                                Err(e) => error!("internal error, {}", e),
                            }
                        }
                        Ok(posts)
                    }
                    Err(e) => Err(e.into()),
                },
                Err(e) => Err(e.into()),
            },
            Err(e) => Err(e.into()),
        }
    }
}
