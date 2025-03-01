use std::borrow::Cow;

use mlua::prelude::*;

pub fn extend_util_library(lua: &Lua, table: LuaTable) -> LuaResult<()> {
    table.raw_set(
        "eval",
        lua.create_function(|lua, (code, options): (LuaString, Option<LuaTable>)| {
            let mut env = None;
            let mut chunk_name: Cow<'static, str> = Cow::Borrowed("=[eval]");

            if let Some((file, line)) = lua
                .inspect_stack(1)
                .and_then(|dbg| dbg.source().short_src.map(|name| (name.into_owned(), dbg.curr_line())))
            {
                chunk_name = Cow::Owned(format!("=[eval@{file}:{line}]"))
            }

            if let Some(options) = options {
                let mut it = options.pairs::<LuaString, LuaValue>();
                while let Some((name, value)) = it.next().transpose()? {
                    match &name.as_bytes()[..] {
                        b"env" => {
                            env = Some(LuaTable::from_lua(value, lua).context("`env` option value must be a table")?);
                        }
                        b"name" => {
                            let lua_name =
                                LuaString::from_lua(value, lua).context("`name` option value must be a string")?;
                            let str_name = lua_name.to_str().context("`name` option value must be valid UTF-8")?;
                            chunk_name = Cow::Owned(format!("={str_name}"));
                            if chunk_name.len() > 128 {
                                return Err(LuaError::runtime("`name` option value is too long"));
                            }
                        }
                        name => {
                            return Err(LuaError::runtime(format!(
                                "`{}` is not a valid eval option name",
                                // TODO: ByteStr instead once stable
                                String::from_utf8_lossy(name)
                            )));
                        }
                    }
                }
            }

            let Some(env) = env else {
                return Err(LuaError::runtime("`env` option missing"));
            };

            lua.load(&code.as_bytes()[..])
                .set_mode(mlua::ChunkMode::Text)
                .set_name(chunk_name)
                .set_environment(env)
                .eval::<LuaMultiValue>()
        })?,
    )?;

    Ok(())
}
