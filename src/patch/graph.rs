use std::sync::Arc;

use crate::{append, xmltree};

const MAX_NODE_INPUTS: usize = 2;

#[derive(Debug, Clone)]
pub enum Value {
    SimpleXml(SimpleXmlDocument),
    AppendScript(Arc<append::Script>),
    Text(Arc<str>),
    Bytes(Arc<[u8]>),
}

#[derive(Debug, Clone)]
pub struct SimpleXmlDocument {
    content: Arc<[xmltree::Node]>,
    had_ftl_root: bool,
}

pub trait ConvertValue: Sized {
    const TYPE_NAME: &'static str;

    fn extract(value: Value) -> Option<Self>;
    fn pack(value: Self) -> Value;
}

macro_rules! impl_value_type {
    ($t: ty => $variant: ident) => {
        impl ConvertValue for $t {
            const TYPE_NAME: &'static str = stringify!($variant);

            fn extract(value: Value) -> Option<Self> {
                match value {
                    Value::$variant(content) => Some(content),
                    _ => None,
                }
            }

            fn pack(value: Self) -> Value {
                Value::$variant(value)
            }
        }
    };
}

impl_value_type!(SimpleXmlDocument => SimpleXml);
impl_value_type!(Arc<append::Script> => AppendScript);
impl_value_type!(Arc<str> => Text);
impl_value_type!(Arc<[u8]> => Bytes);

trait Inputs: Sized {
    fn extract(values: [Option<Value>; 2]) -> Option<Self>;
}

macro_rules! impl_input_type {
    ($($vars: ident),* ; $($none: ident),*) => {
        #[allow(non_snake_case)]
        impl<$($vars: ConvertValue),*> Inputs for ($($vars,)*) {
            fn extract(values: [Option<Value>; 2]) -> Option<Self> {
                let [$(Some($vars),)* $($none),*] = values else {
                    return None;
                };

                Some(($(ConvertValue::extract($vars)?,)*))
            }
        }
    };
}

impl_input_type!(T1 ; None);
impl_input_type!(T1, T2 ; );

trait Node {
    fn run(inputs: impl Inputs) -> impl ConvertValue;
}
