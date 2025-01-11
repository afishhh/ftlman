use std::{collections::HashSet, ffi::c_void, fmt::Write};

use mlua::prelude::*;
use serde::Deserialize;

use super::{LuaArena, LuaExt};

const fn true_fn() -> bool {
    true
}
#[derive(Debug, Clone, Deserialize)]
pub struct PrettyPrintOptions {
    #[serde(default = "true_fn")]
    recursive: bool,
    colors: Option<Colors>,
    indent: Option<String>,
}

impl Default for PrettyPrintOptions {
    fn default() -> Self {
        Self {
            recursive: true,
            colors: None,
            indent: None,
        }
    }
}

#[derive(Debug, Clone, Copy, Deserialize)]
pub enum Colors {
    #[serde(rename = "ansi")]
    Ansi,
}

impl Colors {
    fn start_value(&self, value: &LuaValue) -> &'static str {
        match self {
            Colors::Ansi => match value {
                LuaNil | LuaValue::Boolean(..) => "\x1b[38;5;165m",
                LuaValue::Integer(..) | LuaValue::Number(..) => "\x1b[38;5;33m",
                LuaValue::String(..) => "\x1b[38;5;40m",
                LuaValue::Table(..) => "",
                LuaValue::LightUserData(..) | LuaValue::Function(..) | LuaValue::Thread(..) | LuaValue::Other(..) => {
                    self.start_pointer()
                }
                LuaValue::UserData(_) | LuaValue::Error(..) => "",
            },
        }
    }

    fn start_pointer(&self) -> &'static str {
        match self {
            Colors::Ansi => "\x1b[38;5;227m",
        }
    }

    fn start_error(&self) -> &'static str {
        match self {
            Colors::Ansi => "\x1b[38;5;196m",
        }
    }

    fn reset(&self) -> &'static str {
        match self {
            Colors::Ansi => "\x1b[0m",
        }
    }
}

pub struct PrettyPrinter {
    seen: HashSet<*const c_void>,
    options: PrettyPrintOptions,
}

impl PrettyPrinter {
    pub fn new(options: PrettyPrintOptions) -> Self {
        Self {
            seen: HashSet::new(),
            options,
        }
    }

    fn write_indent(output: &mut impl Write, indent: &str, level: u64) -> std::fmt::Result {
        for _ in 0..level {
            output.write_str(indent)?;
        }

        Ok(())
    }

    fn rec(&mut self, output: &mut impl Write, value: LuaValue, level: u64) -> std::fmt::Result {
        if !value.to_pointer().is_null() && !self.seen.insert(value.to_pointer()) {
            if let Some(colors) = self.options.colors {
                output.write_str(colors.start_pointer())?;
            }

            write!(output, "{}@{:?}", value.type_name(), value.to_pointer())?;

            if let Some(colors) = self.options.colors {
                output.write_str(colors.reset())?;
            }

            return Ok(());
        }

        if let Some(colors) = self.options.colors {
            output.write_str(colors.start_value(&value))?;
        }

        match &value {
            LuaNil => write!(output, "nil"),
            &LuaValue::Boolean(boolean) => {
                if boolean {
                    write!(output, "true")
                } else {
                    write!(output, "false")
                }
            }
            &LuaValue::Integer(integer) => {
                write!(output, "{integer}")
            }
            &LuaValue::Number(number) => {
                write!(output, "{number}")
            }
            LuaValue::String(string) => {
                write!(output, "{string:?}")
            }
            LuaValue::Table(table) => {
                if !self.options.recursive {
                    if let Some(colors) = self.options.colors {
                        output.write_str(colors.start_pointer())?;
                    }

                    write!(output, "table@{:?}", table.to_pointer())
                } else {
                    output.write_char('{')?;
                    let mut it = table.pairs::<LuaValue, LuaValue>();
                    // TODO: Fail gracefully
                    let mut first = true;
                    while let Some((key, value)) = it.next().transpose().unwrap() {
                        if !first {
                            output.write_char(',')?;
                        } else {
                            first = false;
                        }

                        if let Some(indent) = self.options.indent.as_ref() {
                            writeln!(output)?;
                            Self::write_indent(output, indent, level + 1)?;
                        } else {
                            output.write_char(' ')?;
                        }

                        write!(output, "[")?;
                        self.rec(output, key, level + 1)?;
                        write!(output, "]")?;

                        write!(output, " = ")?;
                        self.rec(output, value, level + 1)?;
                    }

                    if !first {
                        if let Some(indent) = self.options.indent.as_ref() {
                            writeln!(output)?;
                            Self::write_indent(output, indent, level)?;
                        } else {
                            output.write_char(' ')?;
                        }
                    }

                    output.write_char('}')?;

                    Ok(())
                }
            }
            LuaValue::LightUserData(..) | LuaValue::Function(..) | LuaValue::Thread(..) => {
                write!(output, "{}@{:?}", value.type_name(), value.to_pointer())
            }
            LuaValue::UserData(any_user_data) => match any_user_data.to_string() {
                Ok(string) => output.write_str(&string),
                Err(error) => {
                    write!(
                        output,
                        "failed to stringify user data@{:?}: ",
                        any_user_data.to_pointer()
                    )?;
                    if let Some(colors) = self.options.colors {
                        output.write_str(colors.start_error())?;
                    }
                    write!(output, "{:?}", error.to_string())
                }
            },
            LuaValue::Other(..) | LuaValue::Error(..) => {
                write!(output, "unknown@{:?}", value.to_pointer())
            }
        }?;

        if let Some(colors) = self.options.colors {
            output.write_str(colors.reset())?;
        }
        Ok(())
    }

    pub fn pretty_print(&mut self, output: &mut impl Write, value: LuaValue) -> std::fmt::Result {
        self.rec(output, value, 0)
    }
}

struct Comparer {}

impl Comparer {
    pub fn new() -> Self {
        Self {}
    }

    // TODO: location tracking
    pub fn compare(&mut self, a: LuaValue, b: LuaValue) -> LuaResult<Result<(), String>> {
        fn compare_simple<T: PartialEq>(a: T, b: T, err: &str) -> LuaResult<Result<(), String>> {
            if a == b {
                Ok(Ok(()))
            } else {
                Ok(Err(err.to_owned()))
            }
        }

        match (&a, &b) {
            (LuaNil, LuaNil) => Ok(Ok(())),
            (LuaValue::Boolean(a), LuaValue::Boolean(b)) => compare_simple(a, b, "booleans have different values"),
            (LuaValue::LightUserData(a), LuaValue::LightUserData(b)) => {
                compare_simple(a, b, "values point to different userdata objects")
            }
            (LuaValue::Integer(a), LuaValue::Integer(b)) => compare_simple(a, b, "integers have different values"),
            (LuaValue::Number(a), LuaValue::Number(b)) => compare_simple(a, b, "numbers have different values"),
            (LuaValue::String(a), LuaValue::String(b)) => compare_simple(a, b, "strings have different values"),
            (LuaValue::Table(a), LuaValue::Table(b)) => {
                let count_a = a.pairs::<LuaValue, LuaValue>().count();
                let mut compared = 0;
                for result in b.pairs::<LuaValue, LuaValue>() {
                    let (key, value_b) = result?;
                    let value_a = a.raw_get::<LuaValue>(key)?;

                    if let Err(e) = self.compare(value_b, value_a)? {
                        return Ok(Err(e));
                    }

                    compared += 1;
                }

                if count_a != compared {
                    return Ok(Err("tables contain a different number of elements".to_owned()));
                }

                Ok(Ok(()))
            }
            (LuaValue::Function(a), LuaValue::Function(b)) => {
                compare_simple(a.to_pointer(), b.to_pointer(), "values point to different functions")
            }
            (LuaValue::Thread(a), LuaValue::Thread(b)) => compare_simple(a, b, "values point to different threads"),
            (LuaValue::UserData(a), LuaValue::UserData(b)) => compare_simple(
                a.to_pointer(),
                b.to_pointer(),
                "values point to different userdata objects",
            ),
            (LuaValue::Error(_), LuaValue::Error(_)) => unreachable!(),
            (LuaValue::Other(..), LuaValue::Other(..)) => {
                compare_simple(a.to_pointer(), b.to_pointer(), "unknown value differs")
            }
            (_, _) => Ok(Err("values have different types".into())),
        }
    }
}

pub fn extend_debug_library(lua: &Lua, table: LuaTable) -> LuaResult<()> {
    #[cfg(debug_assertions)]
    table.set(
        "full_gc",
        lua.create_function(|lua, _: ()| {
            lua.gc_collect()?;
            lua.gc_collect()?;
            let arena = lua.gc();
            println!("gc arena total allocation: {:?}", arena.metrics().total_allocation());
            println!("gc arena allocation debt: {:?}", arena.metrics().allocation_debt());
            lua.app_data_mut::<LuaArena>().unwrap().collect_all();
            Ok(())
        })?,
    )?;

    table.set(
        "pretty_string",
        lua.create_function(|_, (value, options): (LuaValue, LuaValue)| {
            let options = if options.is_nil() {
                PrettyPrintOptions::default()
            } else {
                PrettyPrintOptions::deserialize(mlua::serde::Deserializer::new(options))
                    .context("failed to deserialize argument #2 to mod.debug.pretty_string")?
            };

            let mut output = String::new();
            PrettyPrinter::new(options).pretty_print(&mut output, value).unwrap();

            Ok(output)
        })?,
    )?;

    table.set(
        "pretty_print",
        lua.create_function(|_, (value, options): (LuaValue, LuaValue)| {
            let options = if options.is_nil() {
                PrettyPrintOptions {
                    indent: Some("\t".to_owned()),
                    ..Default::default()
                }
            } else {
                PrettyPrintOptions::deserialize(mlua::serde::Deserializer::new(options))
                    .context("failed to deserialize argument #2 to mod.debug.pretty_print")?
            };

            let mut output = String::new();
            PrettyPrinter::new(options).pretty_print(&mut output, value).unwrap();
            println!("{output}");

            Ok(())
        })?,
    )?;

    table.set(
        "_compare",
        lua.create_function(|_lua, (a, b): (LuaValue, LuaValue)| {
            let mut comparer = Comparer::new();
            Ok(comparer.compare(a, b)?.is_ok())
        })?,
    )?;

    Ok(())
}
