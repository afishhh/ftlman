use std::{
    hash::BuildHasher,
    ops::{Deref, Index},
    ptr::NonNull,
};

#[derive(Debug)]
struct Slot {
    data: NonNull<Path>,
    canonically_cased: bool,
    index: PathIdx,
}

impl Slot {
    unsafe fn path(&self) -> &Path {
        unsafe { self.data.as_ref() }
    }
}

#[derive(Debug)]
pub struct PathInterner {
    hasher: std::hash::RandomState,
    table: hashbrown::HashTable<Slot>,
    paths: Vec<NonNull<Path>>,
}

impl PathInterner {
    pub fn new() -> Self {
        Self {
            hasher: std::hash::RandomState::new(),
            table: hashbrown::HashTable::new(),
            paths: Vec::new(),
        }
    }

    pub fn insert<S>(&mut self, text: S) -> PathIdx
    where
        S: Into<String> + AsRef<str>,
    {
        let hash = self.hasher.hash_one(text.as_ref());
        match self
            .table
            .find_entry(hash, |slot| unsafe { slot.path().as_str() == text.as_ref() })
        {
            Ok(entry) => entry.get().index,
            Err(_) => {
                let index = PathIdx(self.paths.len() as u32);
                let lowercase = text.as_ref().to_lowercase();
                let lowercase_hash = self.hasher.hash_one(lowercase.as_str());
                let canonical_path = if lowercase_hash != hash && lowercase.as_str() != text.as_ref() {
                    // Lowercase repr is not canonical, try a lowercase version too
                    match self.table.find_entry(lowercase_hash, |slot| unsafe {
                        slot.path().as_str() == lowercase.as_str()
                    }) {
                        Ok(entry) => return entry.get().index,
                        Err(_) => {
                            let lowercase_path = Path::convert_boxed_str(lowercase.into_boxed_str());

                            self.table.insert_unique(
                                lowercase_hash,
                                Slot {
                                    data: unsafe { NonNull::new_unchecked(Box::into_raw(lowercase_path)) },
                                    canonically_cased: false,
                                    index,
                                },
                                |slot| self.hasher.hash_one(unsafe { slot.path().as_str() }),
                            );

                            unsafe {
                                NonNull::new_unchecked(Box::into_raw(Path::convert_boxed_str(text.as_ref().into())))
                            }
                        }
                    }
                } else {
                    // Lowercase repr is canonical, no additional entry is necessary
                    unsafe {
                        NonNull::new_unchecked(Box::into_raw(Path::convert_boxed_str(lowercase.into_boxed_str())))
                    }
                };

                // Insert the canonical repr of this Path
                _ = self.table.insert_unique(
                    hash,
                    Slot {
                        data: canonical_path,
                        canonically_cased: true,
                        index,
                    },
                    |slot| self.hasher.hash_one(unsafe { slot.path().as_str() }),
                );

                self.paths.push(canonical_path);

                index
            }
        }
    }
}

impl Drop for PathInterner {
    fn drop(&mut self) {
        for value in std::mem::take(&mut self.table) {
            if !value.canonically_cased {
                drop(unsafe { Box::from_raw(value.data.as_ptr()) });
            }
        }

        for canonical in std::mem::take(&mut self.paths) {
            drop(unsafe { Box::from_raw(canonical.as_ptr()) });
        }
    }
}

impl Index<PathIdx> for PathInterner {
    type Output = Path;

    fn index(&self, index: PathIdx) -> &Self::Output {
        unsafe { self.paths[index.0 as usize].as_ref() }
    }
}

unsafe impl Send for PathInterner {}
unsafe impl Sync for PathInterner {}

#[repr(transparent)]
pub struct Path(str);

impl Path {
    fn convert_boxed_str(boxed: Box<str>) -> Box<Path> {
        // Not sure whether this is guaranteed to be safe but should be
        unsafe { std::mem::transmute(boxed) }
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl Deref for Path {
    type Target = str;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct PathIdx(u32);

impl PathIdx {
    pub const INVALID: Self = PathIdx(u32::MAX);
}

#[cfg(test)]
mod test {
    use super::PathInterner;

    #[test]
    fn path_interner() {
        let mut interner = PathInterner::new();

        let hello_txt = interner.insert("/data/hello.txt");
        let hello_xml = interner.insert("/data/hello.xml");
        let capital_hello_txt = interner.insert("/data/HELLO.txt");

        assert_eq!(hello_txt, capital_hello_txt);
        assert_ne!(hello_xml, capital_hello_txt);

        let capital_test_lua = interner.insert("/data/TEST.lua");
        let test_lua = interner.insert("/data/test.lua");

        assert_eq!(test_lua, capital_test_lua);
        assert_ne!(hello_txt, capital_test_lua);
    }
}
