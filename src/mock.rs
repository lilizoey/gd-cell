use std::{
    collections::HashMap,
    error::Error,
    marker::PhantomData,
    pin::Pin,
    sync::{atomic::AtomicUsize, Mutex, OnceLock},
};

use crate::{guards::NonAliasingGuard, GdCell};

struct InstanceBinding(*mut ());

unsafe impl Sync for InstanceBinding {}
unsafe impl Send for InstanceBinding {}

static INSTANCE_BINDINGS: OnceLock<Mutex<HashMap<usize, InstanceBinding>>> = OnceLock::new();

struct InstanceStorage<T> {
    cell: Pin<Box<GdCell<T>>>,
}

fn binding() -> &'static Mutex<HashMap<usize, InstanceBinding>> {
    INSTANCE_BINDINGS.get_or_init(|| Mutex::new(HashMap::new()))
}

fn register_instance<T>(instance: T) -> usize {
    static COUNTER: AtomicUsize = AtomicUsize::new(0);

    let binding = binding();

    let mut guard = binding.lock().unwrap();

    let key = COUNTER.fetch_add(1, std::sync::atomic::Ordering::AcqRel);

    assert!(!guard.contains_key(&key));

    let cell = Box::pin(GdCell::new(instance));
    let storage = Box::new(InstanceStorage { cell });
    let ptr = Box::into_raw(storage) as *mut ();

    guard.insert(key, InstanceBinding(ptr));
    key
}

unsafe fn free_instance<T>(key: usize) {
    let binding = binding();
    let mut guard = binding.lock().unwrap();

    let InstanceBinding(ptr) = guard.remove(&key).unwrap();

    let ptr: *mut InstanceStorage<T> = ptr as *mut _;

    let storage = unsafe { Box::from_raw(ptr) };
}

unsafe fn get_instance<'a, T>(key: usize) -> &'a InstanceStorage<T> {
    let binding = binding();
    let mut guard = binding.lock().unwrap();

    let instance = guard.get(&key).unwrap();

    let ptr: *mut InstanceStorage<T> = instance.0 as *mut _;

    &*ptr
}

unsafe fn call_immut_method<T>(key: usize, method: fn(&T)) -> Result<(), Box<dyn Error>> {
    let storage = get_instance::<T>(key);

    let instance = storage.cell.as_ref().gd_ref()?;
    method(&*instance);

    Ok(())
}

unsafe fn call_mut_method<T>(key: usize, method: fn(&mut T)) -> Result<(), Box<dyn Error>> {
    let storage = get_instance::<T>(key);

    let mut instance = storage.cell.as_ref().gd_mut()?;
    method(&mut *instance);

    Ok(())
}

struct Base<T> {
    instance_id: usize,
    _p: PhantomData<T>,
}

impl<T> Base<T> {
    fn cell<'a, 'b: 'a>(&'a self) -> Pin<&'b GdCell<T>> {
        let storage = unsafe { get_instance::<T>(self.instance_id) };
        storage.cell.as_ref()
    }
}

struct BaseGuard<'a, T> {
    instance_id: usize,
    non_aliasing_guard: NonAliasingGuard<'a, T>,
}

impl<'a, T> BaseGuard<'a, T> {
    fn new<'b>(instance_id: usize, non_aliasing_guard: NonAliasingGuard<'a, T>) -> Self {
        Self {
            instance_id,
            non_aliasing_guard,
        }
    }

    fn call_immut(&self, f: fn(&T)) {
        unsafe { call_immut_method(self.instance_id, f).unwrap() }
    }

    fn call_mut(&self, f: fn(&mut T)) {
        unsafe { call_mut_method(self.instance_id, f).unwrap() }
    }
}

struct MyClass {
    base: Base<MyClass>,
    int: i64,
}

impl MyClass {
    fn init() -> usize {
        let this = Self {
            base: Base {
                instance_id: 0,
                _p: PhantomData,
            },
            int: 0,
        };
        let key = register_instance(this);

        let instance = unsafe { get_instance::<Self>(key) };
        instance.cell.as_ref().gd_mut().unwrap().base.instance_id = key;
        key
    }

    fn immut_method(&self) {
        println!("immut #1: int is {}", self.int);
    }

    fn mut_method(&mut self) {
        println!("mut #1: int is {}", self.int);
        self.int += 1;
        println!("mut #2: int is now {}", self.int);
    }

    fn mut_method_calls_immut(&mut self) {
        println!("mut_calls_immut #1: int is {}", self.int);
        self.int += 1;
        println!("mut_calls_immut #2: int is now {}", self.int);
        self.base().call_immut(Self::immut_method);
        println!("mut_calls_immut #3: int is now {}", self.int);
    }

    fn mut_method_calls_mut(&mut self) {
        println!("mut_calls_mut #1: int is {}", self.int);
        self.int += 1;
        println!("mut_calls_mut #2: int is now {}", self.int);
        self.base().call_mut(Self::mut_method);
        println!("mut_calls_mut #3: int is now {}", self.int);
    }

    fn mut_method_calls_twice(&mut self) {
        println!("mut_calls_twice #1: int is {}", self.int);
        self.int += 1;
        println!("mut_calls_twice #2: int is now {}", self.int);
        self.base().call_mut(Self::mut_method_calls_immut);
        println!("mut_calls_twice #3: int is now {}", self.int);
    }

    fn mut_method_calls_twice_mut(&mut self) {
        println!("mut_calls_twice_mut #1: int is {}", self.int);
        self.int += 1;
        println!("mut_calls_twice_mut #2: int is now {}", self.int);
        self.base().call_mut(Self::mut_method_calls_mut);
        println!("mut_calls_twice_mut #3: int is now {}", self.int);
    }

    fn immut_calls_immut_directly(&self) {
        println!("immut_calls_directly #1: int is {}", self.int);
        unsafe { call_immut_method(self.base.instance_id, Self::immut_method).unwrap() }
    }

    fn base<'a>(&'a mut self) -> BaseGuard<'a, Self> {
        let cell = self.base.cell();
        BaseGuard::new(self.base.instance_id, cell.set_non_aliasing(self).unwrap())
    }
}

#[test]
fn call_works() {
    let instance_id = MyClass::init();

    unsafe { call_immut_method(instance_id, MyClass::immut_method).unwrap() };
}

#[test]
fn all_calls_work() {
    let instance_id = MyClass::init();

    fn assert_int_is(instance_id: usize, target: i64) {
        let storage = unsafe { get_instance::<MyClass>(instance_id) };
        let bind = storage.cell.as_ref().gd_ref().unwrap();
        assert_eq!(bind.int, target);
    }

    assert_int_is(instance_id, 0);
    unsafe { call_immut_method(instance_id, MyClass::immut_method).unwrap() };
    assert_int_is(instance_id, 0);
    unsafe { call_mut_method(instance_id, MyClass::mut_method).unwrap() };
    assert_int_is(instance_id, 1);
    unsafe { call_mut_method(instance_id, MyClass::mut_method_calls_immut).unwrap() };
    assert_int_is(instance_id, 2);
    unsafe { call_mut_method(instance_id, MyClass::mut_method_calls_mut).unwrap() };
    assert_int_is(instance_id, 4);
    unsafe { call_mut_method(instance_id, MyClass::mut_method_calls_twice).unwrap() };
    assert_int_is(instance_id, 6);
    unsafe { call_mut_method(instance_id, MyClass::mut_method_calls_twice_mut).unwrap() };
    assert_int_is(instance_id, 9);
    unsafe { call_immut_method(instance_id, MyClass::immut_calls_immut_directly).unwrap() };
    assert_int_is(instance_id, 9);
}
