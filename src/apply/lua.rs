use std::{cell::Ref, collections::BTreeMap};

use anyhow::Result;
use luaxmltree::{NodeHeader, NodeRc, RefCellNodeExt as _};
use mlua::{prelude::*, FromLua, UserData, UserDataFields};

use crate::xmltree;

#[path = "lua/xmltree.rs"]
mod luaxmltree;

struct LuaDocument {
    root: LuaElement,
}

impl UserData for LuaDocument {
    fn add_fields<F: UserDataFields<Self>>(fields: &mut F) {
        fields.add_field_method_get("root", |_, this| Ok(this.root.clone()));
    }
    fn add_methods<M: mlua::UserDataMethods<Self>>(methods: &mut M) {}
}

macro_rules! define_as_method {
    ($methods: expr) => {
        $methods.add_method("as", |lua, this, kind: String| {
            let dynamic = this.0.clone() as NodeRc;
            match kind.as_str() {
                "element" => {
                    if dynamic.borrow_header().kind() == luaxmltree::NodeKind::Element {
                        LuaElement(unsafe { NodeHeader::rc_as_concrete_unchecked::<luaxmltree::Element>(dynamic) })
                            .into_lua(lua)
                    } else {
                        Ok(LuaValue::Nil)
                    }
                }
                "text" => {
                    if dynamic.borrow_header().kind() == luaxmltree::NodeKind::Text {
                        LuaText(unsafe {
                            NodeHeader::rc_as_concrete_unchecked::<luaxmltree::SimpleNode<String>>(dynamic)
                        })
                        .into_lua(lua)
                    } else {
                        Ok(LuaValue::Nil)
                    }
                }
                _ => Err(LuaError::runtime("invalid type passed to Node:as cast")),
            }
        })
    };
}

#[derive(Clone, FromLua)]
struct LuaText(luaxmltree::StringRc);

impl UserData for LuaText {
    fn add_fields<F: LuaUserDataFields<Self>>(fields: &mut F) {
        fields.add_field_method_get("type", |_, _| Ok("text"));
        fields.add_field_method_get("content", |_, this| Ok(this.0.borrow().value.to_owned()));
        fields.add_field_method_set("content", |_, this, new: String| {
            this.0.borrow_mut().value = new;
            Ok(())
        });
    }
    fn add_methods<M: LuaUserDataMethods<Self>>(methods: &mut M) {
        define_as_method!(methods);
        methods.add_meta_method("__tostring", |_, this, _: ()| {
            Ok(format!("#text {:?}", &this.0.borrow().value))
        });
    }
}

#[derive(Clone, FromLua)]
struct LuaElement(luaxmltree::ElementRc);

fn node_into_lua(lua: &Lua, node: NodeRc) -> LuaResult<LuaValue> {
    let kind = node.borrow_header().kind();
    match kind {
        luaxmltree::NodeKind::Element => {
            LuaElement(unsafe { NodeHeader::rc_as_concrete_unchecked(node) }).into_lua(lua)
        }
        luaxmltree::NodeKind::Comment => todo!(),
        luaxmltree::NodeKind::CData => todo!(),
        luaxmltree::NodeKind::Text => LuaText(unsafe { NodeHeader::rc_as_concrete_unchecked(node) }).into_lua(lua),
        luaxmltree::NodeKind::ProcessingInstruction => todo!(),
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
            NodeImplicitlyConvertible::Text(text) => unsafe {
                luaxmltree::SimpleNode::create(luaxmltree::NodeKind::Text, text)
            },
        }
    }
}

impl UserData for LuaElement {
    fn add_fields<F: UserDataFields<Self>>(fields: &mut F) {
        fields.add_field_method_get("type", |_, _| Ok("element"));
        fields.add_field_method_get("name", |_, this| Ok(this.0.borrow_mut().name.clone()));
        fields.add_field_method_set("name", |_, this, value: String| {
            this.0.borrow_mut().name = value;
            Ok(())
        });
        fields.add_field_method_get("textContent", |_, this| {
            let this = this.0.borrow();
            let mut output = String::new();
            for child in this.children() {
                if let Some(content) = child.borrow().as_text().map(|x| x.value.as_str()) {
                    output.push_str(content);
                }
            }
            Ok(output)
        });
        fields.add_field_method_set("textContent", |_, this, value: String| {
            let mut this = this.0.borrow_mut();
            this.remove_children();
            this.append_child(unsafe { luaxmltree::SimpleNode::create(luaxmltree::NodeKind::Text, value) });
            Ok(())
        });
    }
    fn add_methods<M: mlua::UserDataMethods<Self>>(methods: &mut M) {
        define_as_method!(methods);
        methods.add_method("append", |_, this, nodes: mlua::Variadic<NodeImplicitlyConvertible>| {
            for node in nodes {
                this.0.borrow_mut().append_child(node.into_node());
            }
            Ok(())
        });
        methods.add_method("children", |_, this, _: ()| Ok(LuaChildren(this.0.borrow().children())));
        methods.add_meta_method("__tostring", |_, this, _: ()| {
            let mut output = String::new();
            this.0.borrow().lua_tostring(&mut output);
            Ok(output)
        });
    }
}

struct LuaChildren(luaxmltree::ElementChildren);

impl UserData for LuaChildren {
    fn add_fields<F: UserDataFields<Self>>(fields: &mut F) {}

    fn add_methods<M: mlua::UserDataMethods<Self>>(methods: &mut M) {
        methods.add_meta_method_mut("__call", |lua, this, _: ()| {
            if let Some(child) = this.0.next() {
                Ok(Some(node_into_lua(lua, child)?))
            } else {
                Ok(None)
            }
        });
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

            Ok(LuaElement(luaxmltree::Element::create(prefix, name, attributes)))
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

    let luatree = unsafe {
        luaxmltree::NodeHeader::rc_as_concrete_unchecked::<luaxmltree::Element>(luaxmltree::convert_into(
            xmltree::Node::Element(std::mem::replace(
                context,
                xmltree::Element {
                    prefix: None,
                    name: String::new(),
                    attributes: BTreeMap::new(),
                    children: Vec::new(),
                },
            )),
        ))
    };

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

    *context = match luaxmltree::convert_from(luatree) {
        xmltree::Node::Element(element) => element,
        _ => unreachable!(),
    };

    Ok(())
}
