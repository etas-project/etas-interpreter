use std::{
    cell::{Ref, RefCell, RefMut},
    fmt,
    rc::Rc,
};

use super::primitive::InterpValue;

#[derive(Clone)]
pub struct ArrayValue(Rc<RefCell<Vec<InterpValue>>>);

impl ArrayValue {
    pub fn new(values: Vec<InterpValue>) -> Self {
        Self(Rc::new(RefCell::new(values)))
    }

    pub fn borrow(&self) -> Ref<'_, Vec<InterpValue>> {
        self.0.borrow()
    }

    pub fn borrow_mut(&self) -> RefMut<'_, Vec<InterpValue>> {
        self.0.borrow_mut()
    }

    pub fn snapshot(&self) -> Vec<InterpValue> {
        self.borrow().clone()
    }

    pub fn make_unique(&mut self) {
        if Rc::strong_count(&self.0) > 1 {
            self.0 = Rc::new(RefCell::new(self.snapshot()));
        }
    }
}

impl fmt::Debug for ArrayValue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("ArrayValue").field(&self.borrow()).finish()
    }
}

impl PartialEq for ArrayValue {
    fn eq(&self, other: &Self) -> bool {
        self.snapshot() == other.snapshot()
    }
}

impl Eq for ArrayValue {}

impl From<Vec<InterpValue>> for ArrayValue {
    fn from(values: Vec<InterpValue>) -> Self {
        Self::new(values)
    }
}

#[derive(Clone)]
pub struct ListValue(Rc<RefCell<Vec<InterpValue>>>);

impl ListValue {
    pub fn new(values: Vec<InterpValue>) -> Self {
        Self(Rc::new(RefCell::new(values)))
    }

    pub fn borrow(&self) -> Ref<'_, Vec<InterpValue>> {
        self.0.borrow()
    }

    pub fn borrow_mut(&self) -> RefMut<'_, Vec<InterpValue>> {
        self.0.borrow_mut()
    }

    pub fn snapshot(&self) -> Vec<InterpValue> {
        self.borrow().clone()
    }

    pub fn make_unique(&mut self) {
        if Rc::strong_count(&self.0) > 1 {
            self.0 = Rc::new(RefCell::new(self.snapshot()));
        }
    }
}

impl fmt::Debug for ListValue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("ListValue").field(&self.borrow()).finish()
    }
}

impl PartialEq for ListValue {
    fn eq(&self, other: &Self) -> bool {
        self.snapshot() == other.snapshot()
    }
}

impl Eq for ListValue {}

impl From<Vec<InterpValue>> for ListValue {
    fn from(values: Vec<InterpValue>) -> Self {
        Self::new(values)
    }
}

#[derive(Clone)]
pub struct SliceValue(Rc<RefCell<Vec<InterpValue>>>);

impl SliceValue {
    pub fn new(values: Vec<InterpValue>) -> Self {
        Self(Rc::new(RefCell::new(values)))
    }

    pub fn borrow(&self) -> Ref<'_, Vec<InterpValue>> {
        self.0.borrow()
    }

    pub fn snapshot(&self) -> Vec<InterpValue> {
        self.borrow().clone()
    }
}

impl fmt::Debug for SliceValue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("SliceValue").field(&self.borrow()).finish()
    }
}

impl PartialEq for SliceValue {
    fn eq(&self, other: &Self) -> bool {
        self.snapshot() == other.snapshot()
    }
}

impl Eq for SliceValue {}

impl From<Vec<InterpValue>> for SliceValue {
    fn from(values: Vec<InterpValue>) -> Self {
        Self::new(values)
    }
}

#[derive(Clone)]
pub struct MapValue(Rc<RefCell<Vec<(InterpValue, InterpValue)>>>);

impl MapValue {
    pub fn new(entries: Vec<(InterpValue, InterpValue)>) -> Self {
        Self(Rc::new(RefCell::new(entries)))
    }

    pub fn borrow(&self) -> Ref<'_, Vec<(InterpValue, InterpValue)>> {
        self.0.borrow()
    }

    pub fn borrow_mut(&self) -> RefMut<'_, Vec<(InterpValue, InterpValue)>> {
        self.0.borrow_mut()
    }

    pub fn snapshot(&self) -> Vec<(InterpValue, InterpValue)> {
        self.borrow().clone()
    }

    pub fn make_unique(&mut self) {
        if Rc::strong_count(&self.0) > 1 {
            self.0 = Rc::new(RefCell::new(self.snapshot()));
        }
    }
}

impl fmt::Debug for MapValue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("MapValue").field(&self.borrow()).finish()
    }
}

impl PartialEq for MapValue {
    fn eq(&self, other: &Self) -> bool {
        self.snapshot() == other.snapshot()
    }
}

impl Eq for MapValue {}

impl From<Vec<(InterpValue, InterpValue)>> for MapValue {
    fn from(entries: Vec<(InterpValue, InterpValue)>) -> Self {
        Self::new(entries)
    }
}

#[derive(Clone)]
pub struct SetValue(Rc<RefCell<Vec<InterpValue>>>);

impl SetValue {
    pub fn new(values: Vec<InterpValue>) -> Self {
        Self(Rc::new(RefCell::new(values)))
    }

    pub fn borrow(&self) -> Ref<'_, Vec<InterpValue>> {
        self.0.borrow()
    }

    pub fn borrow_mut(&self) -> RefMut<'_, Vec<InterpValue>> {
        self.0.borrow_mut()
    }

    pub fn snapshot(&self) -> Vec<InterpValue> {
        self.borrow().clone()
    }

    pub fn make_unique(&mut self) {
        if Rc::strong_count(&self.0) > 1 {
            self.0 = Rc::new(RefCell::new(self.snapshot()));
        }
    }
}

impl fmt::Debug for SetValue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("SetValue").field(&self.borrow()).finish()
    }
}

impl PartialEq for SetValue {
    fn eq(&self, other: &Self) -> bool {
        self.snapshot() == other.snapshot()
    }
}

impl Eq for SetValue {}

impl From<Vec<InterpValue>> for SetValue {
    fn from(values: Vec<InterpValue>) -> Self {
        Self::new(values)
    }
}

#[derive(Clone)]
pub struct RecordValue(Rc<RefCell<Vec<(String, InterpValue)>>>);

impl RecordValue {
    pub fn new(fields: Vec<(String, InterpValue)>) -> Self {
        Self(Rc::new(RefCell::new(fields)))
    }

    pub fn borrow(&self) -> Ref<'_, Vec<(String, InterpValue)>> {
        self.0.borrow()
    }

    pub fn borrow_mut(&self) -> RefMut<'_, Vec<(String, InterpValue)>> {
        self.0.borrow_mut()
    }

    pub fn snapshot(&self) -> Vec<(String, InterpValue)> {
        self.borrow().clone()
    }

    pub fn make_unique(&mut self) {
        if Rc::strong_count(&self.0) > 1 {
            self.0 = Rc::new(RefCell::new(self.snapshot()));
        }
    }
}

impl fmt::Debug for RecordValue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("RecordValue").field(&self.borrow()).finish()
    }
}

impl PartialEq for RecordValue {
    fn eq(&self, other: &Self) -> bool {
        self.snapshot() == other.snapshot()
    }
}

impl Eq for RecordValue {}

impl From<Vec<(String, InterpValue)>> for RecordValue {
    fn from(fields: Vec<(String, InterpValue)>) -> Self {
        Self::new(fields)
    }
}
