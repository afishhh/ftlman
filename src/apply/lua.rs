use std::collections::BTreeMap;

use anyhow::Result;
use mlua::prelude::*;

use crate::xmltree::{
    self,
    dom::{self},
};

mod xml;

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
        xml::create_xml_lib(&lua).context("Failed to create xml library table")?,
    )?;

    #[cfg(debug_assertions)]
    {
        let debug_table = lua.create_table()?;
        debug_table.set(
            "full_gc",
            lua.create_function(|lua, _: ()| {
                lua.gc_collect()?;
                lua.gc_collect()?;
                Ok(())
            })?,
        )?;
        lib_table.set("debug", debug_table)?;
    }

    lua.load("mod = mod.util.readonly(mod)")
        .exec()
        .context("Failed to make builtin mod table read-only")?;

    let env = lua.globals().clone();
    env.set(
        "document",
        lua.create_userdata(xml::LuaDocument {
            root: xml::LuaElement(luatree.clone()),
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
