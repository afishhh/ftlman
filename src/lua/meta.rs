use mlua::prelude::*;

use super::LuaExt;

pub fn create_meta_lib(lua: &mlua::Lua) -> LuaResult<LuaTable> {
    let table = lua.create_protected_table()?;

    let metatable = table.metatable().unwrap();
    metatable.raw_set(
        "__index",
        lua.create_function({
            // FIXME: wait? do these clones actually create cycles?
            //        if so the lua gc would probably have no way to detect them
            let cloned = table.clone();
            move |lua, (_, key): (LuaValue, LuaValue)| {
                if let Some(str) = key.as_string()
                    && str == "current_path"
                {
                    return lua.execution_context().current_file.clone().into_lua(lua);
                }

                cloned.raw_get(key)
            }
        })?,
    )?;

    Ok(table)
}
