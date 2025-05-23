use std::{collections::BTreeMap, fmt::Write as _, ops::Deref};

use gc_arena::{
    lock::{GcRefLock, RefLock},
    DynamicRoot, DynamicRootSet, Gc, Mutation, Rootable,
};
use mlua::{prelude::*, FromLua, UserData, UserDataFields};

use crate::{
    apply::XmlAppendType,
    xmltree::{
        self,
        dom::{
            self, node_insert_after, node_insert_before, CloneMode, ElementChildren, GcElement, GcNode, GcText,
            NodeExt, NodeTraits,
        },
    },
};

use super::{unsize_node, LuaExt};

pub type DynamicElement = DynamicRoot<Rootable![GcElement<'_>]>;

pub struct LuaDocument {
    pub root: LuaElement,
}

fn append_qualified_name(element: &dom::Element, output: &mut String) {
    if let Some(ref prefix) = element.prefix {
        output.push_str(prefix);
        output.push(':');
    }
    output.push_str(&element.name);
}

fn validate_xml_name(name: &str) -> LuaResult<()> {
    const ALLOWED_PUNCTUATION: [char; 2] = ['-', '_'];

    let mut it = name.chars();

    if let Some(illegal) = it
        .next()
        .filter(|c| !c.is_ascii_alphabetic() && !ALLOWED_PUNCTUATION.contains(c))
    {
        return Err(LuaError::runtime(format!(
            "{illegal:?} is not allowed at the start of an XML name"
        )));
    }

    if let Some(illegal) = name
        .chars()
        .find(|c| !c.is_ascii_alphanumeric() && !ALLOWED_PUNCTUATION.contains(c))
    {
        return Err(LuaError::runtime(format!("{illegal:?} is not allowed in an XML name")));
    }

    Ok(())
}

fn element_tostring(element: &dom::Element, output: &mut String) {
    output.push('<');
    append_qualified_name(element, output);
    if !element.attributes.is_empty() {
        for (name, value) in element.attributes.iter() {
            _ = write!(output, " {name:?}={value:?}");
        }
    }

    if element.children().next().is_none() {
        output.push_str("/>");
    } else {
        output.push_str(">...</");
        append_qualified_name(element, output);
        output.push('>');
    }
}

trait LuaNode: Sized {
    unsafe fn get_node<'gc>(&self) -> GcNode<'gc>;
}

macro_rules! impl_lua_node {
    ($type: ident) => {
        impl LuaNode for $type {
            unsafe fn get_node<'gc>(&self) -> GcNode<'gc> {
                unsize_node!(unsafe { *self.0.as_ptr() })
            }
        }
    };
}

fn add_node_fields<T: LuaNode, M: LuaUserDataFields<T>>(fields: &mut M, name: &'static str) {
    fields.add_field("type", name);
    fields.add_field_method_get("previousSibling", |lua, this| {
        lua.gc().mutate(|mc, roots| {
            let mut current = Some(unsafe { this.get_node() });
            std::iter::from_fn(|| {
                if let Some(c) = current {
                    current = c.borrow().previous_sibling();
                    current.and_then(|gc| gc_into_lua(mc, roots, lua, gc))
                } else {
                    None
                }
            })
            .next()
            .transpose()
        })
    });
    fields.add_field_method_get("nextSibling", |lua, this| {
        lua.gc().mutate(|mc, roots| {
            let mut current = Some(unsafe { this.get_node() });
            std::iter::from_fn(|| {
                if let Some(c) = current {
                    current = c.borrow().next_sibling();
                    current.and_then(|gc| gc_into_lua(mc, roots, lua, gc))
                } else {
                    None
                }
            })
            .next()
            .transpose()
        })
    });
    fields.add_field_method_get("parent", |lua, this| {
        lua.gc().mutate(|mc, roots| {
            Ok(unsafe { this.get_node() }
                .borrow()
                .parent()
                .map(|value| roots.stash(mc, value))
                .map(LuaElement))
        })
    });
}

fn check_node_insertion<'gc>(
    mut parent: GcNode<'gc>,
    inserted: GcNode<'gc>,
    arg_number: usize,
    function: &'static str,
) -> LuaResult<()> {
    if Gc::ptr_eq(parent, inserted) {
        return Err(LuaError::runtime(format!(
            "Node passed as argument #{arg_number} to {function} is the parent"
        )));
    }

    loop {
        parent = match parent.borrow().parent() {
            Some(next) => unsize_node!(next),
            None => break,
        };

        if Gc::ptr_eq(parent, inserted) {
            return Err(LuaError::runtime(format!(
                "Node passed as argument #{arg_number} to {function} is an ancestor of the parent"
            )));
        }
    }

    if inserted.borrow().parent().is_some() {
        return Err(LuaError::runtime(format!(
            "Node passed as argument #{arg_number} to {function} already has a parent"
        )));
    }

    Ok(())
}

fn add_node_methods<T: LuaNode, M: LuaUserDataMethods<T>>(methods: &mut M) {
    methods.add_method("as", |lua, this, kind: String| {
        lua.gc().mutate(|mc, roots| {
            let dynamic = unsafe { this.get_node() };
            match kind.as_str() {
                "element" => {
                    if let Some(value) = dom::Element::downcast_gc(dynamic) {
                        LuaElement(roots.stash(mc, value)).into_lua(lua)
                    } else {
                        Ok(LuaValue::Nil)
                    }
                }
                "text" => {
                    if let Some(value) = dom::Text::downcast_gc(dynamic) {
                        LuaText(roots.stash(mc, value)).into_lua(lua)
                    } else {
                        Ok(LuaValue::Nil)
                    }
                }
                _ => Err(LuaError::runtime("invalid type passed to Node:as cast")),
            }
        })
    });

    methods.add_method("detach", |lua, this, _: ()| {
        lua.gc().mutate(|mc, _| {
            detach_any(mc, unsafe { this.get_node() });
        });
        Ok(())
    });

    // TODO: factor this stuff out into a generic function or smth
    methods.add_method(
        "before",
        |lua, this, nodes: mlua::Variadic<NodeImplicitlyConvertible>| {
            lua.gc().mutate(|mc, _| {
                let this = unsafe { this.get_node() };
                let Some(parent) = this.borrow().parent() else {
                    return Err(LuaError::runtime("cannot insert nodes before orphan node"));
                };

                for (i, node) in nodes.into_iter().enumerate() {
                    let node = node.into_node(mc);
                    check_node_insertion(unsize_node!(parent), node, i + 1, "Element:before")?;
                    node_insert_before(this, mc, node);
                }

                Ok(())
            })
        },
    );

    methods.add_method(
        "after",
        |lua, this, nodes: mlua::Variadic<NodeImplicitlyConvertible>| {
            lua.gc().mutate(|mc, _| {
                let this = unsafe { this.get_node() };
                let Some(parent) = this.borrow().parent() else {
                    return Err(LuaError::runtime("cannot insert nodes after orphan node"));
                };

                for (i, node) in nodes.into_iter().enumerate().rev() {
                    let node = node.into_node(mc);
                    check_node_insertion(unsize_node!(parent), node, i + 1, "Element:after")?;
                    node_insert_after(this, mc, node);
                }

                Ok(())
            })
        },
    );

    methods.add_method("clone", |lua, this, mode: Option<LuaString>| {
        let mode = match mode.as_ref().map(|s| s.as_bytes()).as_deref() {
            Some(b"shallow") => CloneMode::Shallow,
            Some(b"deep") | None => CloneMode::Deep,
            Some(other) => {
                return Err(LuaError::runtime(format!(
                    "`{}` is not a valid clone mode",
                    String::from_utf8_lossy(other)
                )))
            }
        };

        lua.gc().mutate(|mc, roots| {
            let this = unsafe { this.get_node() };
            gc_into_lua(mc, roots, lua, dom::clone_any(mc, this, mode)).transpose()
        })
    });
}

impl UserData for LuaDocument {
    fn add_fields<F: UserDataFields<Self>>(fields: &mut F) {
        fields.add_field_method_get("root", |_, this| Ok(this.root.clone()));
    }
    fn add_methods<M: mlua::UserDataMethods<Self>>(_methods: &mut M) {}
}

#[derive(Clone, FromLua)]
#[repr(transparent)]
struct LuaText(DynamicRoot<Rootable![GcText<'_>]>);

impl_lua_node!(LuaText);

impl UserData for LuaText {
    fn add_fields<F: LuaUserDataFields<Self>>(fields: &mut F) {
        add_node_fields(fields, "text");
        fields.add_field_method_get("content", |_, this| {
            Ok(unsafe { *this.0.as_ptr() }.borrow().content.to_owned())
        });
        fields.add_field_method_set("content", |_, this, new: String| {
            unsafe { (*this.0.as_ptr()).as_ref_cell() }.borrow_mut().content = new;
            Ok(())
        });
    }
    fn add_methods<M: LuaUserDataMethods<Self>>(methods: &mut M) {
        add_node_methods(methods);
        methods.add_meta_method("__tostring", |_, this, _: ()| {
            Ok(format!("#text {:?}", &unsafe { *this.0.as_ptr() }.borrow().content))
        });
    }
}

#[derive(Clone, FromLua)]
#[repr(transparent)]
pub struct LuaElement(pub DynamicElement);

impl_lua_node!(LuaElement);

fn gc_into_lua<'gc>(
    mc: &Mutation<'gc>,
    roots: &DynamicRootSet<'gc>,
    lua: &Lua,
    node: GcNode<'gc>,
) -> Option<LuaResult<LuaValue>> {
    let kind = node.borrow().kind();

    match kind {
        dom::NodeKind::Element => {
            Some(LuaElement(roots.stash(mc, unsafe { dom::Element::downcast_gc_unchecked(node) })).into_lua(lua))
        }
        dom::NodeKind::Text => {
            Some(LuaText(roots.stash(mc, unsafe { dom::Text::downcast_gc_unchecked(node) })).into_lua(lua))
        }
        // TODO:
        _ => None,
    }
}

// static dispatch :rocket::rocket::rocket:
fn detach_any<'gc>(mc: &Mutation<'gc>, node: GcNode<'gc>) {
    let kind = node.borrow().kind();

    macro_rules! downcast_and_detach {
        ($type: ty) => {
            unsafe { <$type>::downcast_gc_unchecked(node) }
                .borrow_mut(mc)
                .detach(mc)
        };
    }

    match kind {
        dom::NodeKind::Element => downcast_and_detach!(dom::Element),
        dom::NodeKind::Comment => downcast_and_detach!(dom::Comment),
        dom::NodeKind::CData => downcast_and_detach!(dom::CData),
        dom::NodeKind::Text => downcast_and_detach!(dom::Text),
    }
}

#[derive(Clone)]
enum LuaConcreteNode {
    Element(LuaElement),
    Text(LuaText),
}

#[derive(Clone)]
enum NodeImplicitlyConvertible {
    Node(LuaConcreteNode),
    String(String),
}

impl FromLua for NodeImplicitlyConvertible {
    fn from_lua(value: mlua::Value, lua: &mlua::Lua) -> mlua::Result<Self> {
        let type_name = value.type_name();
        match LuaConcreteNode::from_lua(value.clone(), lua) {
            Ok(element) => Ok(Self::Node(element)),
            Err(_) => match String::from_lua(value, lua) {
                Ok(content) => Ok(Self::String(content)),
                Err(_) => Err(mlua::Error::FromLuaConversionError {
                    from: type_name,
                    to: "XML Node".to_string(),
                    message: None,
                }),
            },
        }
    }
}

impl FromLua for LuaConcreteNode {
    fn from_lua(value: mlua::Value, lua: &mlua::Lua) -> mlua::Result<Self> {
        let type_name = value.type_name();
        match LuaElement::from_lua(value.clone(), lua) {
            Ok(element) => Ok(Self::Element(element)),
            Err(_) => match LuaText::from_lua(value, lua) {
                Ok(content) => Ok(Self::Text(content)),
                Err(_) => Err(mlua::Error::FromLuaConversionError {
                    from: type_name,
                    to: "XML Node".to_string(),
                    message: None,
                }),
            },
        }
    }
}

impl NodeImplicitlyConvertible {
    fn into_node<'gc>(self, mc: &Mutation<'gc>) -> GcNode<'gc> {
        match self {
            NodeImplicitlyConvertible::Node(e) => e.into_node(mc),
            NodeImplicitlyConvertible::String(text) => unsize_node!(dom::Text::create(mc, text)),
        }
    }
}

impl LuaConcreteNode {
    fn into_node<'gc>(self, _mc: &Mutation<'gc>) -> GcNode<'gc> {
        match self {
            LuaConcreteNode::Element(e) => unsize_node!(unsafe { *e.0.as_ptr() }),
            LuaConcreteNode::Text(t) => unsize_node!(unsafe { *t.0.as_ptr() }),
        }
    }
}

impl LuaElement {
    unsafe fn get<'gc>(&self) -> dom::GcElement<'gc> {
        unsafe { *self.0.as_ptr() }
    }
}

impl UserData for LuaElement {
    fn add_fields<F: UserDataFields<Self>>(fields: &mut F) {
        add_node_fields(fields, "element");

        fields.add_field_method_get("name", |_, this| Ok(unsafe { *this.0.as_ptr() }.borrow().name.clone()));
        fields.add_field_method_set("name", |_, this, value: Box<str>| {
            // SAFETY: No write barrier has to be triggered as no Gc pointers are modified.
            validate_xml_name(&value)?;
            unsafe { this.get().as_ref_cell() }.borrow_mut().name = value;
            Ok(())
        });
        fields.add_field_method_get("prefix", |_, this| {
            Ok(unsafe { *this.0.as_ptr() }.borrow().prefix.clone())
        });
        fields.add_field_method_set("prefix", |_, this, value: Option<Box<str>>| {
            // SAFETY: See above
            if let Some(pfx) = value.as_ref() {
                validate_xml_name(pfx)?;
            }
            unsafe { this.get().as_ref_cell() }.borrow_mut().prefix = value;
            Ok(())
        });

        fields.add_field_method_get("attrs", |_, this| {
            Ok(LuaAttributes {
                element: this.clone(),
                raw: false,
            })
        });

        fields.add_field_method_get("rawattrs", |_, this| {
            Ok(LuaAttributes {
                element: this.clone(),
                raw: true,
            })
        });

        fields.add_field_method_get("textContent", |_, this| {
            let this = unsafe { *this.0.as_ptr() };
            let mut output = String::new();
            for child in this.borrow().descendants() {
                if let Some(content) = dom::Text::downcast_ref(&*child.borrow()).map(|x| x.content.as_str()) {
                    output.push_str(content);
                }
            }
            Ok(output)
        });

        fields.add_field_method_set("textContent", |lua, this, value: String| {
            lua.gc().mutate(|mc, roots| {
                let mut this = roots.fetch(&this.0).borrow_mut(mc);
                this.remove_children(mc);
                this.append_child(mc, unsize_node!(dom::Text::create(mc, value)));
            });
            Ok(())
        });
    }

    fn add_methods<M: mlua::UserDataMethods<Self>>(methods: &mut M) {
        add_node_methods(methods);

        methods.add_method(
            "prepend",
            |lua, this, nodes: mlua::Variadic<NodeImplicitlyConvertible>| {
                lua.gc().mutate(|mc, roots| {
                    let this = *roots.fetch(&this.0);
                    for (i, node) in nodes.into_iter().enumerate().rev() {
                        let node = node.into_node(mc);
                        check_node_insertion(unsize_node!(this), node, i + 1, "Element:prepend")?;
                        this.borrow_mut(mc).prepend_child(mc, node);
                    }
                    Ok(())
                })
            },
        );

        methods.add_method(
            "append",
            |lua, this, nodes: mlua::Variadic<NodeImplicitlyConvertible>| {
                lua.gc().mutate(|mc, roots| {
                    let this = *roots.fetch(&this.0);
                    for (i, node) in nodes.into_iter().enumerate() {
                        let node = node.into_node(mc);
                        check_node_insertion(unsize_node!(this), node, i + 1, "Element:append")?;
                        this.borrow_mut(mc).append_child(mc, node);
                    }
                    Ok(())
                })
            },
        );

        methods.add_method("firstElementChild", |lua, this, _: ()| {
            Ok(lua.gc().mutate(|mc, roots| {
                roots
                    .fetch(&this.0)
                    .borrow()
                    .children()
                    .find_map(dom::Element::downcast_gc)
                    .map(|element| LuaElement(roots.stash(mc, element)))
            }))
        });

        methods.add_method("lastElementChild", |lua, this, _: ()| {
            Ok(lua.gc().mutate(|mc, roots| {
                roots
                    .fetch(&this.0)
                    .borrow()
                    .children()
                    .rev()
                    .find_map(dom::Element::downcast_gc)
                    .map(|element| LuaElement(roots.stash(mc, element)))
            }))
        });

        methods.add_method("firstChild", |lua, this, _: ()| {
            lua.gc().mutate(|mc, roots| {
                roots
                    .fetch(&this.0)
                    .borrow()
                    .children()
                    .find_map(|node| gc_into_lua(mc, roots, lua, node))
                    .transpose()
            })
        });

        methods.add_method("firstChild", |lua, this, _: ()| {
            lua.gc().mutate(|mc, roots| {
                roots
                    .fetch(&this.0)
                    .borrow()
                    .children()
                    .rev()
                    .find_map(|node| gc_into_lua(mc, roots, lua, node))
                    .transpose()
            })
        });

        methods.add_method("children", |lua, this, _: ()| {
            Ok(lua.gc().mutate(|mc, roots| {
                LuaElementChildren(LuaChildNodes(
                    roots.stash(mc, Gc::new(mc, RefLock::new(roots.fetch(&this.0).borrow().children()))),
                ))
            }))
        });

        methods.add_method("childNodes", |lua, this, _: ()| {
            Ok(lua.gc().mutate(|mc, roots| {
                LuaChildNodes(roots.stash(mc, Gc::new(mc, RefLock::new(roots.fetch(&this.0).borrow().children()))))
            }))
        });

        methods.add_meta_method("__tostring", |_, this, _: ()| {
            let mut output = String::new();
            element_tostring(unsafe { &this.get().borrow() }, &mut output);
            Ok(output)
        });

        #[cfg(debug_assertions)]
        methods.add_method("debug", |_, this, _: ()| {
            let mut output = String::new();
            write!(output, "{:#?}", unsafe { this.get().borrow() }).unwrap();
            Ok(output)
        });
    }
}

#[derive(Clone)]
struct LuaAttributes {
    element: LuaElement,
    /// Whether to parse values like "10", "3.14", or "true" into the appriopriate
    /// datatype automatically.
    raw: bool,
}

impl LuaAttributes {
    fn process_value(&self, lua: &Lua, value: &str) -> LuaResult<LuaValue> {
        if self.raw {
            lua.create_string(value).map(LuaValue::String)
        } else {
            match value {
                "true" => return Ok(LuaValue::Boolean(true)),
                "false" => return Ok(LuaValue::Boolean(false)),
                _ => (),
            };

            if let Ok(integer) = LuaInteger::from_str_radix(value, 10) {
                Ok(LuaValue::Integer(integer))
            } else if let Ok(number) = value.parse() {
                Ok(LuaValue::Number(number))
            } else {
                lua.create_string(value).map(LuaValue::String)
            }
        }
    }

    fn into_iterator(self, lua: &Lua) -> LuaResult<LuaFunction> {
        let mut current = unsafe { self.element.get().borrow() }
            .attributes
            .first_key_value()
            .map(|(key, _)| key.clone());

        lua.create_function_mut(move |lua, _: ()| {
            if let Some(previous) = current.take() {
                let e = unsafe { self.element.get().borrow() };
                let mut it = e.attributes.range(previous..);
                match it.next() {
                    Some((key, value)) => {
                        let lua_key = lua.create_string(&**key)?;
                        current = it.next().map(|(key, _)| key.clone());
                        (lua_key, self.process_value(lua, value)?).into_lua_multi(lua)
                    }
                    None => LuaValue::Nil.into_lua_multi(lua),
                }
            } else {
                LuaValue::Nil.into_lua_multi(lua)
            }
        })
    }
}

impl UserData for LuaAttributes {
    fn add_methods<M: LuaUserDataMethods<Self>>(methods: &mut M) {
        methods.add_meta_method("__call", |lua, this, this2: LuaUserDataRef<LuaElement>| {
            if !std::ptr::eq(this.element.0.as_ptr(), this2.deref().0.as_ptr()) {
                return Err(LuaError::runtime("LuaAttributes called with mismatched self pointer"));
            }

            this.clone().into_iterator(lua)
        });

        methods.add_meta_method_mut("__index", |lua, this, key: LuaString| {
            let Ok(key) = key.to_str() else {
                return Ok(LuaValue::Nil);
            };

            let element = unsafe { *this.element.0.as_ptr() }.borrow();
            let Some(value) = element.attributes.get(&*key) else {
                return Ok(LuaValue::Nil);
            };

            this.process_value(lua, value)
        });

        methods.add_meta_method_mut("__newindex", |_, this, (key, value): (Box<str>, LuaValue)| {
            validate_xml_name(&key)?;

            let gc = unsafe { this.element.get() };
            let mut element = unsafe { gc.as_ref_cell().borrow_mut() };

            if value.is_nil() {
                element.attributes.remove(&key);
                return Ok(());
            }

            let string_value: Box<str> = if this.raw || value.is_string() {
                if let LuaValue::String(string) = value {
                    if let Ok(value) = string.to_str() {
                        (*value).into()
                    } else {
                        return Err(LuaError::runtime("invalid UTF-8 assigned to attribute"));
                    }
                } else {
                    return Err(LuaError::runtime(format!(
                        "cannot assign {} to raw element attribute",
                        value.type_name()
                    )));
                }
            } else {
                match value {
                    LuaValue::Boolean(value) => match value {
                        true => "true".into(),
                        false => "false".into(),
                    },
                    LuaValue::Integer(value) => itoa::Buffer::new().format(value).into(),
                    LuaValue::Number(value) => ryu::Buffer::new().format(value).into(),
                    other => {
                        return Err(LuaError::runtime(format!(
                            "cannot assign {} to element attribute",
                            other.type_name()
                        )));
                    }
                }
            };

            element.attributes.insert(key, string_value);

            Ok(())
        });
    }
}

#[repr(transparent)]
#[allow(clippy::type_complexity)]
struct LuaChildNodes(DynamicRoot<Rootable![GcRefLock<'_, ElementChildren<'_>>]>);

impl UserData for LuaChildNodes {
    fn add_fields<F: UserDataFields<Self>>(_fields: &mut F) {}

    fn add_methods<M: mlua::UserDataMethods<Self>>(methods: &mut M) {
        methods.add_meta_method_mut("__call", |lua, this, _: ()| {
            lua.gc()
                .mutate(|mc, roots| {
                    let mut iter = roots.fetch(&this.0).borrow_mut(mc);
                    iter.find_map(|node| gc_into_lua(mc, roots, lua, node))
                })
                .transpose()
        });
    }
}

#[repr(transparent)]
struct LuaElementChildren(LuaChildNodes);

impl UserData for LuaElementChildren {
    fn add_fields<F: UserDataFields<Self>>(_fields: &mut F) {}

    fn add_methods<M: mlua::UserDataMethods<Self>>(methods: &mut M) {
        methods.add_meta_method_mut("__call", |lua, this, _: ()| {
            Ok(lua.gc().mutate(|mc, roots| {
                let mut iter = roots.fetch(&this.0 .0).borrow_mut(mc);
                iter.find_map(|node| {
                    dom::Element::downcast_gc(node).map(|element| LuaElement(roots.stash(mc, element)))
                })
            }))
        });
    }
}

pub fn create_xml_lib(lua: &Lua) -> LuaResult<LuaTable> {
    let table = lua.create_protected_table()?;

    table.raw_set(
        "element",
        lua.create_function(|lua, args: LuaMultiValue| {
            // clippy moment (long type), may be slightly more readable to have it side by side like this though
            type Args = (Option<Box<str>>, Box<str>, Option<BTreeMap<Box<str>, Box<str>>>);
            let (prefix, name, attributes): Args = match FromLuaMulti::from_lua_multi(args.clone(), lua) {
                Ok(result) => result,
                Err(_) => {
                    // TODO: error message doesn't mention previous overload
                    let (name, attributes) = FromLuaMulti::from_lua_multi(args, lua)?;
                    (None, name, attributes)
                }
            };

            if let Some(pfx) = prefix.as_ref() {
                validate_xml_name(pfx)?;
            }
            validate_xml_name(&name)?;
            if let Some(attrs) = attributes.as_ref() {
                for key in attrs.keys() {
                    validate_xml_name(key)?;
                }
            }

            Ok(LuaElement(lua.gc().mutate(|mc, roots| {
                roots.stash(
                    mc,
                    dom::Element::create(mc, prefix, name, Option::unwrap_or_default(attributes)),
                )
            })))
        })?,
    )?;

    table.raw_set(
        "parse",
        lua.create_function(|lua, xml: LuaString| {
            lua.gc().mutate(|mc, roots| {
                let xml_bytes = xml.as_bytes();
                let xml_text = std::str::from_utf8(&xml_bytes)
                    .into_lua_err()
                    .context("input XML must be valid UTF-8")?;
                let unwrapped = crate::apply::unwrap_xml_text(xml_text);

                let mut result = LuaMultiValue::new();
                let nodes = dom::Element::parse_all(mc, &unwrapped).into_lua_err()?;
                for node in nodes {
                    if let Some(value) = gc_into_lua(mc, roots, lua, node).transpose()? {
                        result.push_back(value);
                    }
                }
                Ok(result)
            })
        })?,
    )?;

    table.raw_set(
        "stringify",
        lua.create_function(|lua, nodes: mlua::Variadic<NodeImplicitlyConvertible>| {
            lua.gc().mutate(|mc, _| {
                let mut writer = speedy_xml::Writer::new(std::io::Cursor::new(Vec::new()));

                for node in nodes {
                    xmltree::emitter::write_node(&mut writer, &dom::DomTreeEmitter, node.into_node(mc))
                        .into_lua_err()
                        .context("Failed to write node")?;
                }

                Ok(lua.create_string(writer.finish()?.into_inner()))
            })
        })?,
    )?;

    table.raw_set(
        "append",
        lua.create_function(
            |lua, (lower, upper, options): (LuaString, LuaString, Option<LuaTable>)| {
                let mut kind = XmlAppendType::Append;

                if let Some(options) = options {
                    let mut it = options.pairs::<LuaString, LuaValue>();
                    while let Some((name, value)) = it.next().transpose()? {
                        match &name.as_bytes()[..] {
                            b"type" => {
                                let lua_type =
                                    LuaString::from_lua(value, lua).context("`type` option value must be a string")?;
                                kind = match &lua_type.as_bytes()[..] {
                                    b"xml.append" => XmlAppendType::Append,
                                    b"xml.rawappend" => XmlAppendType::RawAppend,
                                    _ => {
                                        return Err(LuaError::runtime(format!(
                                            "`{}` is not a valid append type",
                                            // TODO: ByteStr instead once stable
                                            String::from_utf8_lossy(&lua_type.as_bytes()[..])
                                        )));
                                    }
                                };
                            }
                            name => {
                                return Err(LuaError::runtime(format!(
                                    "`{}` is not a valid append option name",
                                    // TODO: ByteStr instead once stable
                                    String::from_utf8_lossy(name)
                                )));
                            }
                        }
                    }
                }

                let lower_bytes = lower.as_bytes();
                let lower = std::str::from_utf8(&lower_bytes[..])
                    .map_err(LuaError::runtime)
                    .context("Failed to decode document")?;
                let upper_bytes = upper.as_bytes();
                let upper = std::str::from_utf8(&upper_bytes[..])
                    .map_err(LuaError::runtime)
                    .context("Failed to decode patch")?;
                let result = crate::apply::apply_one_xml(lower, upper, kind, None).map_err(LuaError::runtime)?;

                Ok(lua.create_string(result))
            },
        )?,
    )?;

    Ok(table)
}
