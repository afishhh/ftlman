use std::{cell::Ref, collections::BTreeMap, fmt::Write as _};

use anyhow::Result;
use mlua::{prelude::*, FromLua, UserData, UserDataFields};

use crate::xmltree::{
    self,
    dom::{self, NodeRc, NodeTraits as _, RcCell},
};

struct LuaDocument {
    root: LuaElement,
}

fn append_qualified_name(element: &dom::Element, output: &mut String) {
    if let Some(ref prefix) = element.prefix {
        output.push_str(prefix);
        output.push(':');
    }
    output.push_str(&element.name);
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
    fn as_node(&self) -> Ref<dyn dom::Node>;
    fn to_node(&self) -> NodeRc;
}

macro_rules! impl_lua_node {
    ($type: ident) => {
        impl LuaNode for $type {
            fn as_node(&self) -> Ref<dyn dom::Node> {
                self.0.borrow()
            }

            fn to_node(&self) -> NodeRc {
                self.0.clone()
            }
        }
    };
}

fn add_node_fields<T: LuaNode, M: LuaUserDataFields<T>>(fields: &mut M, name: &'static str) {
    fields.add_field("type", name);
    fields.add_field_method_get("previousSibling", |_, this| {
        Ok(this.as_node().previous_sibling().cloned().map(NodeIntoLua))
    });
    fields.add_field_method_get("nextSibling", |_, this| {
        Ok(this.as_node().next_sibling().cloned().map(NodeIntoLua))
    });
}

fn add_node_methods<T: LuaNode, M: LuaUserDataMethods<T>>(methods: &mut M) {
    methods.add_method("as", |lua, this, kind: String| {
        let dynamic = this.to_node().clone();
        match kind.as_str() {
            "element" => {
                if let Ok(value) = dom::Element::downcast_rc(dynamic) {
                    LuaElement(value).into_lua(lua)
                } else {
                    Ok(LuaValue::Nil)
                }
            }
            "text" => {
                if let Ok(value) = dom::Text::downcast_rc(dynamic) {
                    LuaText(value).into_lua(lua)
                } else {
                    Ok(LuaValue::Nil)
                }
            }
            _ => Err(LuaError::runtime("invalid type passed to Node:as cast")),
        }
    })
}

impl UserData for LuaDocument {
    fn add_fields<F: UserDataFields<Self>>(fields: &mut F) {
        fields.add_field_method_get("root", |_, this| Ok(this.root.clone()));
    }
    fn add_methods<M: mlua::UserDataMethods<Self>>(_methods: &mut M) {}
}

#[derive(Clone, FromLua)]
#[repr(transparent)]
struct LuaText(RcCell<dom::Text>);

impl_lua_node!(LuaText);

impl UserData for LuaText {
    fn add_fields<F: LuaUserDataFields<Self>>(fields: &mut F) {
        add_node_fields(fields, "text");
        fields.add_field_method_get("content", |_, this| Ok(this.0.borrow().content.to_owned()));
        fields.add_field_method_set("content", |_, this, new: String| {
            this.0.borrow_mut().content = new;
            Ok(())
        });
    }
    fn add_methods<M: LuaUserDataMethods<Self>>(methods: &mut M) {
        add_node_methods(methods);
        methods.add_meta_method("__tostring", |_, this, _: ()| {
            Ok(format!("#text {:?}", &this.0.borrow().content))
        });
    }
}

#[derive(Clone, FromLua)]
#[repr(transparent)]
struct LuaElement(dom::ElementRc);

impl_lua_node!(LuaElement);

#[repr(transparent)]
struct NodeIntoLua(NodeRc);

impl IntoLua for NodeIntoLua {
    fn into_lua(self, lua: &Lua) -> LuaResult<LuaValue> {
        let node = self.0;
        let kind = node.borrow().kind();
        match kind {
            dom::NodeKind::Element => LuaElement(unsafe { dom::Element::downcast_rc_unchecked(node) }).into_lua(lua),
            dom::NodeKind::Comment => todo!(),
            dom::NodeKind::CData => todo!(),
            dom::NodeKind::Text => LuaText(unsafe { dom::Text::downcast_rc_unchecked(node) }).into_lua(lua),
            dom::NodeKind::ProcessingInstruction => todo!(),
        }
    }
}

#[derive(Clone)]
enum NodeImplicitlyConvertible {
    Element(LuaElement),
    Text(String),
}

impl FromLua for NodeImplicitlyConvertible {
    fn from_lua(value: mlua::Value, lua: &mlua::Lua) -> mlua::Result<Self> {
        let type_name = value.type_name();
        match LuaElement::from_lua(value.clone(), lua) {
            Ok(element) => Ok(Self::Element(element)),
            Err(_) => match String::from_lua(value, lua) {
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
    fn into_node(self) -> NodeRc {
        match self {
            NodeImplicitlyConvertible::Element(e) => e.0,
            NodeImplicitlyConvertible::Text(text) => dom::Text::create(text),
        }
    }
}

impl UserData for LuaElement {
    fn add_fields<F: UserDataFields<Self>>(fields: &mut F) {
        add_node_fields(fields, "element");

        fields.add_field_method_get("name", |_, this| Ok(this.0.borrow_mut().name.clone()));
        fields.add_field_method_set("name", |_, this, value: String| {
            this.0.borrow_mut().name = value;
            Ok(())
        });
        fields.add_field_method_get("prefix", |_, this| Ok(this.0.borrow_mut().prefix.clone()));
        fields.add_field_method_set("prefix", |_, this, value: Option<String>| {
            this.0.borrow_mut().prefix = value;
            Ok(())
        });

        fields.add_field_method_get("textContent", |_, this| {
            let this = this.0.borrow();
            let mut output = String::new();
            for child in this.children() {
                if let Some(content) = dom::Text::downcast_ref(&*child.borrow()).map(|x| x.content.as_str()) {
                    output.push_str(content);
                }
            }
            Ok(output)
        });
        fields.add_field_method_set("textContent", |_, this, value: String| {
            let mut this = this.0.borrow_mut();
            this.remove_children();
            this.append_child(dom::Text::create(value));
            Ok(())
        });
    }
    fn add_methods<M: mlua::UserDataMethods<Self>>(methods: &mut M) {
        add_node_methods(methods);
        methods.add_method("append", |_, this, nodes: mlua::Variadic<NodeImplicitlyConvertible>| {
            for node in nodes {
                this.0.borrow_mut().append_child(node.into_node());
            }
            Ok(())
        });
        methods.add_method("children", |_, this, _: ()| {
            Ok(LuaIterator(this.0.borrow().children().filter_map(|value| {
                dom::Element::downcast_rc(value).ok().map(LuaElement)
            })))
        });
        methods.add_method("childNodes", |_, this, _: ()| {
            Ok(LuaIterator(this.0.borrow().children().map(NodeIntoLua)))
        });
        methods.add_meta_method("__tostring", |_, this, _: ()| {
            let mut output = String::new();
            element_tostring(&this.0.borrow(), &mut output);
            Ok(output)
        });
    }
}

#[repr(transparent)]
struct LuaIterator<T>(T);

impl<T: Iterator> UserData for LuaIterator<T>
where
    T::Item: IntoLua,
{
    fn add_fields<F: UserDataFields<Self>>(_fields: &mut F) {}

    fn add_methods<M: mlua::UserDataMethods<Self>>(methods: &mut M) {
        methods.add_meta_method_mut("__call", |_, this, _: ()| Ok(this.0.next()));
    }
}

fn create_xml_lib(lua: &Lua) -> LuaResult<LuaTable> {
    let table = lua.create_table()?;

    table.set(
        "element",
        lua.create_function(|lua, args: LuaMultiValue| {
            let (prefix, name, attributes) = match FromLuaMulti::from_lua_multi(args.clone(), lua) {
                Ok(result) => result,
                Err(_) => {
                    // TODO: error message doesn't mention previous overload
                    let (name, attributes) = FromLuaMulti::from_lua_multi(args, lua)?;
                    (None, name, attributes)
                }
            };

            Ok(LuaElement(dom::Element::create(prefix, name, attributes)))
        })?,
    )?;

    Ok(table)
}

pub fn lua_append(context: &mut xmltree::Element, code: &str) -> Result<()> {
    let lua = mlua::Lua::new_with(
        mlua::StdLib::COROUTINE | mlua::StdLib::TABLE | mlua::StdLib::STRING | mlua::StdLib::MATH,
        mlua::LuaOptions::new(),
    )
    .context("Failed to initialize Lua")?;

    let luatree = dom::Element::from_tree(
        std::mem::replace(
            context,
            xmltree::Element {
                prefix: None,
                name: String::new(),
                attributes: BTreeMap::new(),
                children: Vec::new(),
            },
        ),
        None,
    );

    let lib_table = lua.create_table()?;
    lua.globals().set("mod", lib_table.clone())?;

    lua.load(include_str!("./lua/util.lua"))
        .set_name("<FTLMAN BUILTIN>/util.lua")
        .exec()
        .context("Failed to execute builtin util.lua script")?;

    lua.load(include_str!("./lua/iterutil.lua"))
        .set_name("<FTLMAN BUILTIN>/iterutil.lua")
        .exec()
        .context("Failed to execute builtin iterutil.lua script")?;

    lib_table.set(
        "xml",
        create_xml_lib(&lua).context("Failed to create xml library table")?,
    )?;

    lua.load("mod = mod.util.readonly(mod)")
        .exec()
        .context("Failed to make builtin mod table read-only")?;

    let env = lua.globals().clone();
    env.set(
        "document",
        lua.create_userdata(LuaDocument {
            root: LuaElement(luatree.clone()),
        })?,
    )?;
    lua.load(code)
        .set_name("<PATCH>")
        .set_mode(mlua::ChunkMode::Text)
        .set_environment(env)
        .exec()
        .context("Failed to execute lua append script")?;

    *context = match dom::to_tree(luatree) {
        xmltree::Node::Element(element) => element,
        _ => unreachable!(),
    };

    Ok(())
}
