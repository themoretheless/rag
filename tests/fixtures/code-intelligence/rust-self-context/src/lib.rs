pub struct Worker<'a> {
    data: &'a [u8],
}
impl<'a> Worker<'a> {
    pub fn finish(&self) -> u32 {
        3
    }
    pub fn run(&self) -> u32 {
        self.finish()
    }
    pub fn copy(&self) -> Self {
        Self { data: self.data }
    }
}
pub fn check(worker: Worker<'_>) -> u32 {
    worker.copy().finish()
}

pub struct Item;
impl Item {
    pub fn finish(&self) -> u32 {
        1
    }
}
pub fn root_exercise(root: Item) -> u32 {
    root.finish()
}

pub mod inside {
    pub struct Item;
    impl Item {
        pub fn finish(&self) -> u32 {
            4
        }
    }
    pub fn exercise(inline: Item) -> u32 {
        inline.finish()
    }
}

pub struct Service;
impl Service {
    pub fn finish(&self) -> u32 {
        99
    }
}
pub mod providers {
    pub struct Service;
    impl Service {
        pub fn finish(&self) -> u32 {
            2
        }
    }
}
pub mod nested {
    pub struct Worker<'a> {
        data: &'a [u8],
        service: super::providers::Service,
    }
    impl<'a> Worker<'a> {
        pub fn run(&self) -> u32 {
            self.service.finish()
        }
    }
    pub fn exercise(data: &[u8]) -> u32 {
        let worker = Worker {
            data,
            service: super::providers::Service,
        };
        worker.run()
    }
}

#[test]
fn owner_identity() {
    let worker = Worker { data: b"abc" };
    assert_eq!(worker.run(), 3);
    assert_eq!(check(worker), 3);
    assert_eq!(root_exercise(Item), 1);
    assert_eq!(inside::exercise(inside::Item), 4);
    assert_eq!(nested::exercise(b"abc"), 2);
}
