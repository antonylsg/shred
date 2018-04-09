use std::any::TypeId;
use std::marker::PhantomData;

use fxhash::FxHashMap;
use mopa::Any;

use {Resource, Resources};

/// Helper trait for the `MetaTable`.
/// This trait is required to be implemented for a trait to be compatible with the meta table.
///
/// # Examples
///
/// ```
/// use shred::CastFrom;
///
/// trait Foo {
///     fn foo1(&self);
///     fn foo2(&mut self, x: i32) -> i32;
/// }
///
/// impl<T> CastFrom<T> for Foo
/// where
///     T: Foo + 'static,
/// {
///     fn cast(t: &T) -> &(Foo + 'static) {
///         t
///     }
///
///     fn cast_mut(t: &mut T) -> &mut (Foo + 'static) {
///         t
///     }
/// }
/// ```
pub trait CastFrom<T> {
    /// Casts an immutable `T` reference to a trait object.
    fn cast(t: &T) -> &Self;

    /// Casts a mutable `T` reference to a trait object.
    fn cast_mut(t: &mut T) -> &mut Self;
}

/// An iterator for the `MetaTable`.
pub struct MetaIter<'a, T: ?Sized + 'a> {
    fat: &'a [Fat],
    index: usize,
    res: &'a mut Resources,
    tys: &'a [TypeId],
    // `MetaIter` is invariant over `T`
    marker: PhantomData<*mut T>,
}

impl<'a, T> Iterator for MetaIter<'a, T>
where
    T: ?Sized + 'a,
{
    type Item = &'a T;

    fn next(&mut self) -> Option<<Self as Iterator>::Item> {
        use std::mem::transmute;

        let index = self.index;
        self.index += 1;

        // Ugly hack that works due to `UnsafeCell` and distinct resources.
        unsafe {
            transmute::<&mut Resources, &'a mut Resources>(&mut self.res)
                .get_mut_raw(match self.tys.get(index) {
                    Some(&x) => x,
                    None => return None,
                })
                .map(|res| self.fat[index].create_ptr::<T>(&*res as *const _ as *const ()))
                .map(|ptr| &*ptr)
                .or_else(|| self.next())
        }
    }
}

struct Fat(usize);

impl Fat {
    pub unsafe fn from_ptr<T: ?Sized>(t: &T) -> Self {
        use std::ptr::read;

        assert_unsized::<T>();

        let fat_ptr = &t as *const &T as *const usize;
        // Memory layout:
        // [object pointer, vtable pointer]
        //  ^^^^^^^^^^^^^^  ^^^^^^^^^^^^^^
        //  8 bytes       | 8 bytes
        // (on 32-bit both have 4 bytes)
        let vtable = read::<usize>(fat_ptr.offset(1));

        Fat(vtable)
    }

    pub unsafe fn create_ptr<T: ?Sized>(&self, ptr: *const ()) -> *const T {
        let fat_ptr: (*const (), usize) = (ptr, self.0);

        *(&fat_ptr as *const (*const (), usize) as *const *const T)
    }
}

/// A mutable iterator for the `MetaTable`.
pub struct MetaIterMut<'a, T: ?Sized + 'a> {
    fat: &'a [Fat],
    index: usize,
    res: &'a mut Resources,
    tys: &'a [TypeId],
    // `MetaIterMut` is invariant over `T`
    marker: PhantomData<*mut T>,
}

impl<'a, T> Iterator for MetaIterMut<'a, T>
where
    T: ?Sized + 'a,
{
    type Item = &'a mut T;

    fn next(&mut self) -> Option<<Self as Iterator>::Item> {
        use std::mem::transmute;

        let index = self.index;
        self.index += 1;

        // Ugly hack that works due to `UnsafeCell` and distinct resources.
        unsafe {
            transmute::<&mut Resources, &'a mut Resources>(&mut self.res)
                .get_mut_raw(match self.tys.get(index) {
                    Some(&x) => x,
                    None => return None,
                })
                .map(|res| self.fat[index].create_ptr::<T>(res as *mut _ as *const ()) as *mut T)
                .map(|ptr| &mut *ptr)
                .or_else(|| self.next())
        }
    }
}

/// The `MetaTable` which allows to store object-safe trait implementations for resources.
///
/// For example, you have a trait `Foo` that is implemented by several resources.
/// You can register all the implementors using `MetaTable::register`. Later on, you
/// can iterate over all resources that implement `Foo` without knowing their specific type.
///
/// # Examples
///
/// ```
/// use shred::{CastFrom, MetaTable, Resources};
///
/// trait Object {
///     fn method1(&self) -> i32;
///
///     fn method2(&mut self, x: i32);
/// }
///
/// impl<T> CastFrom<T> for Object
/// where
///     T: Object + 'static,
/// {
///     fn cast(t: &T) -> &Self {
///         t
///     }
///
///     fn cast_mut(t: &mut T) -> &mut Self {
///         t
///     }
/// }
///
/// struct ImplementorA(i32);
///
/// impl Object for ImplementorA {
///     fn method1(&self) -> i32 {
///         self.0
///     }
///
///     fn method2(&mut self, x: i32) {
///         self.0 += x;
///     }
/// }
///
/// struct ImplementorB(i32);
///
/// impl Object for ImplementorB {
///     fn method1(&self) -> i32 {
///         self.0
///     }
///
///     fn method2(&mut self, x: i32) {
///         self.0 *= x;
///     }
/// }
///
/// let mut res = Resources::new();
///
/// res.insert(ImplementorA(3));
/// res.insert(ImplementorB(1));
///
/// let mut table = MetaTable::<Object>::new();
/// table.register(&ImplementorA(31415)); // Can just be some instance of type `&ImplementorA`.
/// table.register(&ImplementorB(27182));
///
/// {
///     let mut iter = table.iter(&mut res);
///     assert_eq!(iter.next().unwrap().method1(), 3);
///     assert_eq!(iter.next().unwrap().method1(), 1);
/// }
/// ```
pub struct MetaTable<T: ?Sized> {
    fat: Vec<Fat>,
    indices: FxHashMap<TypeId, usize>,
    tys: Vec<TypeId>,
    // `MetaTable` is invariant over `T`
    marker: PhantomData<*mut T>,
}

impl<T: ?Sized> MetaTable<T> {
    /// Creates a new `MetaTable`.
    pub fn new() -> Self {
        assert_unsized::<T>();

        Default::default()
    }

    /// Registers a resource `R` that implements the trait `T`.
    /// This just needs some instance of type `R` to retrieve the vtable.
    /// It doesn't have to be the same object you're calling `get` with later.
    pub fn register<R>(&mut self, r: &R)
    where
        R: Resource,
        T: CastFrom<R> + 'static,
    {
        use std::collections::hash_map::Entry;

        let fat = unsafe { Fat::from_ptr(<T as CastFrom<R>>::cast(r)) };

        let ty_id = TypeId::of::<R>();

        // Important: ensure no entry exists twice!
        let len = self.indices.len();
        match self.indices.entry(ty_id) {
            Entry::Occupied(occ) => {
                let ind = *occ.get();

                self.fat[ind] = fat;
            }
            Entry::Vacant(vac) => {
                vac.insert(len);

                self.fat.push(fat);
                self.tys.push(ty_id);
            }
        }
    }

    /// Tries to convert `res` to a trait object of type `&T`.
    /// If `res` doesn't have an implementation for `T` (or it wasn't registered),
    /// this will return `None`.
    pub fn get<'a>(&self, res: &'a Resource) -> Option<&'a T> {
        unsafe {
            self.indices
                .get(&Any::get_type_id(res))
                .map(move |&ind| &*self.fat[ind].create_ptr(res as *const _ as *const ()))
        }
    }

    /// Tries to convert `res` to a trait object of type `&mut T`.
    /// If `res` doesn't have an implementation for `T` (or it wasn't registered),
    /// this will return `None`.
    pub fn get_mut<'a>(&self, res: &'a mut Resource) -> Option<&'a mut T> {
        unsafe {
            self.indices.get(&Any::get_type_id(res)).map(move |&ind| {
                &mut *(self.fat[ind].create_ptr::<T>(res as *const _ as *const ()) as *mut T)
            })
        }
    }

    /// Iterates all resources that implement `T` and were registered.
    pub fn iter<'a>(&'a self, res: &'a mut Resources) -> MetaIter<'a, T> {
        MetaIter {
            fat: &self.fat,
            index: 0,
            res,
            tys: &self.tys,
            marker: PhantomData,
        }
    }

    /// Iterates all resources that implement `T` and were registered mutably.
    pub fn iter_mut<'a>(&'a self, res: &'a mut Resources) -> MetaIterMut<'a, T> {
        MetaIterMut {
            fat: &self.fat,
            index: 0,
            res,
            tys: &self.tys,
            marker: PhantomData,
        }
    }
}

impl<T> Default for MetaTable<T>
where
    T: ?Sized,
{
    fn default() -> Self {
        MetaTable {
            fat: Default::default(),
            indices: Default::default(),
            tys: Default::default(),
            marker: Default::default(),
        }
    }
}

fn assert_unsized<T: ?Sized>() {
    use std::mem::size_of;

    assert_eq!(size_of::<&T>(), 2 * size_of::<usize>());
}

#[cfg(test)]
mod tests {
    use super::*;
    use Resources;

    trait Object {
        fn method1(&self) -> i32;

        fn method2(&mut self, x: i32);
    }

    impl<T> CastFrom<T> for Object
    where
        T: Object + 'static,
    {
        fn cast(t: &T) -> &Self {
            t
        }

        fn cast_mut(t: &mut T) -> &mut Self {
            t
        }
    }

    struct ImplementorA(i32);

    impl Object for ImplementorA {
        fn method1(&self) -> i32 {
            self.0
        }

        fn method2(&mut self, x: i32) {
            self.0 += x;
        }
    }

    struct ImplementorB(i32);

    impl Object for ImplementorB {
        fn method1(&self) -> i32 {
            self.0
        }

        fn method2(&mut self, x: i32) {
            self.0 *= x;
        }
    }

    #[test]
    fn test_iter_all() {
        let mut res = Resources::new();

        res.insert(ImplementorA(3));
        res.insert(ImplementorB(1));

        let mut table = MetaTable::<Object>::new();
        table.register(&ImplementorA(125));
        table.register(&ImplementorB(111111));

        {
            let mut iter = table.iter(&mut res);
            assert_eq!(iter.next().unwrap().method1(), 3);
            assert_eq!(iter.next().unwrap().method1(), 1);
        }

        {
            let mut iter_mut = table.iter_mut(&mut res);
            let obj = iter_mut.next().unwrap();
            obj.method2(3);
            assert_eq!(obj.method1(), 6);
            let obj = iter_mut.next().unwrap();
            obj.method2(4);
            assert_eq!(obj.method1(), 4);
        }
    }
}
