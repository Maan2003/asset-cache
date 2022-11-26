#![allow(dead_code)]
use std::{
    any::Any, collections::HashMap, marker::PhantomData, num::NonZeroUsize, ops::Deref, sync::Arc,
};

use lru::LruCache;

pub struct ResourceCache {
    in_use: HashMap<String, RawHandle>,
    loaded: LruCache<String, RawHandle>,
}

#[derive(Clone, Debug)]
pub struct RawHandle(Arc<HandleInner<dyn Any + Send + Sync>>);

impl RawHandle {
    fn ptr_eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
}

#[derive(Debug)]
struct HandleInner<T: ?Sized> {
    key: String,
    value: T,
}

// Invariant: type in RawHandle is T
#[derive(Debug)]
pub struct Handle<T: ?Sized> {
    raw: RawHandle,
    ty: PhantomData<*const T>,
}

impl<T: ?Sized> Clone for Handle<T> {
    fn clone(&self) -> Self {
        Self {
            raw: self.raw.clone(),
            ty: self.ty.clone(),
        }
    }
}

impl<T: Send + Sync + 'static> Handle<T> {
    fn new(key: String, value: T) -> Self {
        Self {
            raw: RawHandle(Arc::new(HandleInner { key, value })),
            ty: PhantomData,
        }
    }
}

impl<T: Send + Sync + 'static> Into<RawHandle> for Handle<T> {
    fn into(self) -> RawHandle {
        self.raw
    }
}

impl<T: Send + Sync + 'static> Deref for Handle<T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        // use unsafe here?
        &self.raw.0.value.downcast_ref().unwrap()
    }
}

impl RawHandle {
    pub fn downcast<T: Send + Sync + 'static>(self) -> Result<Handle<T>, RawHandle> {
        if self.0.value.is::<T>() {
            Ok(Handle {
                raw: self,
                ty: PhantomData,
            })
        } else {
            Err(self)
        }
    }
}

impl ResourceCache {
    pub fn new(capacity: NonZeroUsize) -> Self {
        Self {
            in_use: HashMap::new(),
            loaded: LruCache::new(capacity),
        }
    }

    pub fn insert<T: Send + Sync + 'static>(&mut self, key: String, value: T) -> Handle<T> {
        let _ = self.loaded.pop(&key);
        let handle = Handle::new(key.clone(), value);
        self.in_use.insert(key, handle.clone().into());
        handle
    }

    pub fn get<T: Send + Sync + 'static>(&mut self, key: &str) -> Option<Handle<T>> {
        self.get_raw(key).and_then(|x| x.downcast().ok())
    }

    pub fn get_raw(&mut self, key: &str) -> Option<RawHandle> {
        match self.in_use.get(key) {
            Some(value) => Some(value.clone()),
            None => match self.loaded.pop(key) {
                Some(value) => {
                    self.in_use.insert(key.to_owned(), value.clone());
                    Some(value)
                }
                None => None,
            },
        }
    }

    pub fn remove(&mut self, value: RawHandle) {
        // this value and one stored in in_use map
        if Arc::strong_count(&value.0) == 2 {
            self.in_use.remove(&value.0.key);
            self.loaded.put(value.0.key.clone(), value);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn create() {
        let res = ResourceCache::new(NonZeroUsize::new(2).unwrap());
        assert!(res.in_use.is_empty());
        assert!(res.loaded.is_empty());
    }

    #[test]
    fn insert_first() {
        let mut res = ResourceCache::new(NonZeroUsize::new(2).unwrap());
        let _ = res.insert(String::from("test"), 1);
        assert_eq!(res.in_use.len(), 1);
        assert!(res.loaded.is_empty());
    }

    #[test]
    fn insert_deref() {
        let mut res = ResourceCache::new(NonZeroUsize::new(2).unwrap());
        let asset = res.insert(String::from("test"), 1);
        assert_eq!(*asset, 1);
    }

    #[test]
    fn insert_no_extra_clones() {
        let mut res = ResourceCache::new(NonZeroUsize::new(2).unwrap());
        let asset = res.insert(String::from("test"), 1);
        assert_eq!(Arc::strong_count(&asset.raw.0), 2);
        assert_eq!(Arc::weak_count(&asset.raw.0), 0);
    }

    #[test]
    fn insert_twice() {
        let mut res = ResourceCache::new(NonZeroUsize::new(2).unwrap());
        let _ = res.insert(String::from("test"), 1);
        let _ = res.insert(String::from("test2"), 2);
        assert_eq!(res.in_use.len(), 2);
        assert!(res.loaded.is_empty());
    }

    #[test]
    fn insert_and_get() {
        let mut res = ResourceCache::new(NonZeroUsize::new(2).unwrap());
        let asset = res.insert(String::from("test"), 1);
        let asset2 = res.get_raw("test").unwrap();
        assert!(asset.raw.ptr_eq(&asset2));
    }

    #[test]
    fn insert_and_overwrite() {
        let mut res = ResourceCache::new(NonZeroUsize::new(2).unwrap());
        let asset1 = res.insert(String::from("test"), 1);
        let asset2 = res.insert(String::from("test"), 2);
        let asset3 = res.get_raw("test").unwrap();
        assert_eq!(*asset1, 1);
        assert_eq!(*asset2, 2);
        assert!(asset2.raw.ptr_eq(&asset3));
    }

    #[test]
    fn remove() {
        let mut res = ResourceCache::new(NonZeroUsize::new(2).unwrap());
        let asset1 = res.insert(String::from("test"), 1);
        res.remove(asset1.raw);
        assert_eq!(res.in_use.len(), 0);
        assert_eq!(res.loaded.len(), 1);
    }

    #[test]
    fn remove_get() {
        let mut res = ResourceCache::new(NonZeroUsize::new(2).unwrap());
        let asset1 = res.insert(String::from("test"), 1);
        res.remove(asset1.raw);
        assert_eq!(res.in_use.len(), 0);
        assert_eq!(res.loaded.len(), 1);
        assert!(res.get_raw("test").is_some());
        assert_eq!(res.in_use.len(), 1);
        assert_eq!(res.loaded.len(), 0);
    }

    #[test]
    fn remove_multiple() {
        let mut res = ResourceCache::new(NonZeroUsize::new(2).unwrap());
        let asset1 = res.insert(String::from("test"), 1);
        let asset2 = res.insert(String::from("test2"), 2);
        let asset3 = res.insert(String::from("test3"), 3);
        res.remove(asset1.raw);
        res.remove(asset2.raw);
        assert_eq!(res.in_use.len(), 1);
        res.remove(asset3.raw);
        assert_eq!(res.in_use.len(), 0);
        assert_eq!(res.loaded.len(), 2);
        assert!(res.get_raw("test").is_none());
        assert_eq!(*res.get::<i32>("test2").unwrap(), 2);
        assert_eq!(*res.get::<i32>("test3").unwrap(), 3);
    }

    #[test]
    fn remove_overwrite() {
        let mut res = ResourceCache::new(NonZeroUsize::new(2).unwrap());
        let asset1 = res.insert(String::from("test"), 1);
        res.remove(asset1.raw);
        assert_eq!(res.in_use.len(), 0);
        assert_eq!(res.loaded.len(), 1);
        let asset2 = res.insert(String::from("test"), 3);
        assert_eq!(res.in_use.len(), 1);
        assert_eq!(res.loaded.len(), 0);
        assert_eq!(*res.get::<i32>("test").unwrap(), 3);
    }
}
