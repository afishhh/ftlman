use std::{
    borrow::Cow,
    sync::{
        atomic::{AtomicPtr, Ordering},
        OnceLock,
    },
};

use fluent::{concurrent::FluentBundle, FluentArgs, FluentMessage, FluentResource};
use lazy_static::lazy_static;
use log::{error, warn};
use parking_lot::Mutex;
use std::collections::HashMap;

const LOCALE_DEFINITIONS: &[(&str, &str)] = &[
    ("en", include_str!("../i18n/en.ftl")),
    ("pl", include_str!("../i18n/pl.ftl")),
];
const FALLBACK_LOCALE: &str = "en";

struct Localiser {
    current_locale: AtomicPtr<&'static str>,
    bundles: HashMap<&'static str, FluentBundle<FluentResource>>,
}

static LOCALISER: OnceLock<Localiser> = OnceLock::new();

#[cfg(target_family = "unix")]
fn get_system_language() -> Option<String> {
    use std::env::VarError;

    const VAR_CHAIN: &[&str] = &["LC_MESSAGES", "LC_ALL", "LANG", "LANGUAGE"];

    VAR_CHAIN
        .iter()
        .filter_map(|x| match std::env::var(x) {
            Err(VarError::NotPresent) => None,
            other => Some(other),
        })
        .next()
        .map(|result| {
            let value = result.expect("Failed to decode a locale environment variable");
            let end_index = match (value.find("_"), value.find(".")) {
                (None, None) => value.len(),
                (None, Some(i)) => i,
                (Some(i), None) => i,
                (Some(a), Some(b)) => std::cmp::min(a, b),
            };
            value[..end_index].to_string()
        })
}

#[cfg(target_family = "windows")]
fn get_system_language() -> Option<String> {
    let mut locale_name = unsafe {
        let mut buffer = [0u16; 64];
        let len = winapi::um::winnls::GetUserDefaultLocaleName(buffer.as_mut_ptr(), buffer.len() as i32) as usize;
        match String::from_utf16(&buffer[..len]) {
            Ok(ok) => ok,
            Err(e) => {
                log::warn!(
                    "GetUserDefaultLocaleName returned invalid utf16 string {:?}: {e}",
                    &buffer[..len]
                );
                return None;
            }
        }
    };

    match locale_name.split_once('-') {
        Some(x) => {
            locale_name.truncate(x.0.len());
            Some(locale_name)
        }
        None => {
            log::warn!("GetUserDefaultLocaleName returned a string without a '-': {locale_name:?}");
            None
        }
    }
}

#[cfg(not(any(target_family = "windows", target_family = "unix")))]
fn get_system_language() -> Option<String> {
    None
}

pub fn get_locale_pointer(name: &str) -> Option<*mut &'static str> {
    LOCALE_DEFINITIONS
        .iter()
        .filter_map(|(sname, _)| {
            if *sname == name {
                Some(sname as *const _ as *mut _)
            } else {
                None
            }
        })
        .next()
}

pub fn current_language() -> &'static str {
    unsafe { *LOCALISER.get().unwrap().current_locale.load(Ordering::Acquire) }
}

pub fn init() {
    LOCALISER
        .set(Localiser {
            current_locale: AtomicPtr::new(
                get_system_language()
                    .as_deref()
                    .and_then(|name| match get_locale_pointer(name) {
                        Some(value) => Some(value),
                        None => {
                            warn!("Failed to get locale for system language {name}");
                            None
                        }
                    })
                    .unwrap_or(&LOCALE_DEFINITIONS[0].0 as *const _ as *mut _),
            ),
            bundles: LOCALE_DEFINITIONS
                .iter()
                .map(|(name, ftl)| {
                    (*name, {
                        let res = FluentResource::try_new(ftl.to_string()).unwrap();
                        let mut bundle = FluentBundle::new_concurrent(vec![name.parse().unwrap()]);
                        bundle.add_resource(res).unwrap();
                        bundle
                    })
                })
                .collect(),
        })
        .map_err(drop)
        .unwrap();
}

lazy_static! {
    static ref MISSING_STRINGS: Mutex<HashMap<(&'static str, &'static str), &'static str>> = Mutex::new(HashMap::new());
}

fn notify_missing(locale: &'static str, id: &str) -> (&'static str, bool) {
    let mut lock = MISSING_STRINGS.lock();
    if let Some(value) = lock.get(&(locale, id)) {
        (value, false)
    } else {
        let value = id.to_string().leak();
        lock.insert((locale, value), value);
        (value, true)
    }
}

enum LookupResult {
    Success(&'static FluentBundle<FluentResource>, FluentMessage<'static>),
    // Contains id interned in MISSING_STRINGS and leaked for 'static
    Missing(&'static str),
}

fn lookup(id: &str) -> LookupResult {
    let localiser = LOCALISER.get().unwrap();
    let current = unsafe { *localiser.current_locale.load(Ordering::Acquire) };
    let bundle = &localiser.bundles[current];
    match bundle.get_message(id) {
        Some(message) => LookupResult::Success(bundle, message),
        None => {
            let (interned, first) = notify_missing(current, id);
            let fallback_bundle = &localiser.bundles[FALLBACK_LOCALE];
            match fallback_bundle.get_message(id) {
                Some(message) => {
                    if first {
                        warn!("String {id} not found in {current} but found in {FALLBACK_LOCALE}")
                    }
                    LookupResult::Success(fallback_bundle, message)
                }
                None => {
                    if notify_missing(FALLBACK_LOCALE, id).1 {
                        error!("String {id} not found");
                    }
                    LookupResult::Missing(interned)
                }
            }
        }
    }
}

pub fn resolve(id: &str, args: Option<&FluentArgs>) -> Cow<'static, str> {
    let (bundle, message) = match lookup(id) {
        LookupResult::Success(bundle, message) => (bundle, message),
        LookupResult::Missing(interned) => return Cow::Borrowed(interned),
    };
    let mut errors = vec![];
    let result = bundle.format_pattern(message.value().unwrap(), args, &mut errors);
    for error in errors {
        error!("{}", error)
    }
    result
}

#[macro_export]
macro_rules! l {
    ($id: literal) => {
        $crate::i18n::resolve($id, None)
    };
    ($id: literal, $($tt: tt)*) => {
        $crate::i18n::resolve($id, Some(&fluent::fluent_args! {
            $($tt)*
        }))
    };
}
