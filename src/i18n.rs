use std::{
    borrow::Cow,
    env::VarError,
    sync::{
        atomic::{AtomicPtr, Ordering},
        OnceLock,
    },
};

use eframe::egui::{
    ahash::{HashMap, HashMapExt},
    mutex::Mutex,
};
use fluent::{concurrent::FluentBundle, FluentArgs, FluentMessage, FluentResource};
use lazy_static::lazy_static;
use log::{error, warn};

const LOCALE_DEFINITIONS: &[(&str, &str)] = &[
    ("en", include_str!("../i18n/en.ftl")),
];
const FALLBACK_LOCALE: &str = "en";

struct Localiser {
    current_locale: AtomicPtr<&'static str>,
    bundles: HashMap<&'static str, FluentBundle<FluentResource>>,
}

static LOCALISER: OnceLock<Localiser> = OnceLock::new();

fn get_language_from_env() -> Option<String> {
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

pub fn init() {
    LOCALISER
        .set(Localiser {
            current_locale: AtomicPtr::new(
                get_language_from_env()
                    .as_deref()
                    .and_then(|name| match get_locale_pointer(name) {
                        Some(value) => Some(value),
                        None => {
                            warn!(
                                "Failed to get locale for language {name} deduced from environment"
                            );
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
    static ref MISSING_STRINGS: Mutex<HashMap<(&'static str, &'static str), &'static str>> =
        Mutex::new(HashMap::new());
}

fn notify_missing(locale: &'static str, id: &str) -> (&'static str, bool) {
    let mut lock = MISSING_STRINGS.lock();
    if let Some(value) = lock.get(&(locale, id)) {
        return (value, false);
    } else {
        let value = id.to_string().leak();
        lock.insert((locale, value), value);
        return (value, true);
    }
}

enum LookupResult {
    Success(
        &'static FluentBundle<FluentResource>,
        FluentMessage<'static>,
    ),
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
