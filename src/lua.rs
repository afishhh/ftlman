use gc_arena::{DynamicRootSet, Rootable};
use mlua::prelude::*;

use crate::xmltree::{
    self,
    dom::{self, unsize_node},
};

mod debug;
mod xml;

type LuaArena = gc_arena::Arena<Rootable![DynamicRootSet<'_>]>;

trait LuaExt {
    fn gc(&self) -> mlua::AppDataRef<LuaArena>;
}

impl LuaExt for Lua {
    fn gc(&self) -> mlua::AppDataRef<LuaArena> {
        self.app_data_ref::<LuaArena>()
            .expect("lua object should contain a dynamic gc arena")
    }
}

pub struct ModLuaRuntime {
    lua: Lua,
}

pub struct LuaContext {
    pub document_root: Option<xmltree::Element>,
    pub print_arena_stats: bool,
}

impl ModLuaRuntime {
    pub fn new() -> LuaResult<Self> {
        let lua = mlua::Lua::new_with(
            mlua::StdLib::TABLE | mlua::StdLib::STRING | mlua::StdLib::MATH,
            mlua::LuaOptions::new(),
        )
        .context("Failed to initialize Lua")?;

        lua.globals().raw_remove("dofile")?;
        lua.globals().raw_remove("require")?;

        // This function causes HRTB deduction problems so no, I cannot replace the closure.
        #[allow(clippy::redundant_closure)]
        let arena: LuaArena = LuaArena::new(|mc| DynamicRootSet::new(mc));

        lua.set_app_data(arena);

        let lib_table = lua.create_table()?;
        lua.globals().set("mod", lib_table.clone())?;

        macro_rules! load_builtin_lib {
            ($filename: literal) => {
                lua.load(include_str!(concat!("lua/", $filename)))
                    .set_name(concat!("<BUILTIN>/", $filename))
                    .exec()
                    .context(concat!("Failed to execute builtin ", $filename, " script"))?;
            };
        }

        load_builtin_lib!("util.lua");
        load_builtin_lib!("iterutil.lua");
        load_builtin_lib!("table.lua");
        load_builtin_lib!("debug.lua");

        lib_table.set(
            "xml",
            xml::create_xml_lib(&lua).context("Failed to create xml library table")?,
        )?;

        debug::extend_debug_library(&lua, lib_table.get::<LuaTable>("debug")?)
            .context("Failed to load debug builtins")?;

        lua.load("mod = mod.util.readonly(mod)")
            .exec()
            .context("Failed to make builtin mod table read-only")?;

        Ok(Self { lua })
    }

    pub fn run(&mut self, code: &str, filename: &str, context: &mut LuaContext) -> LuaResult<()> {
        let luatree = {
            let arena = self.lua.gc();
            let root = context.document_root.take();

            root.map(|element| arena.mutate(|mc, roots| roots.stash(mc, dom::Element::from_tree(mc, element, None))))
        };

        let lua = &self.lua;

        lua.load("mod = mod.util.readonly(mod)")
            .exec()
            .context("Failed to make builtin mod table read-only")?;

        let env = lua.globals().clone();
        if let Some(ref root) = luatree {
            env.set(
                "document",
                lua.create_userdata(xml::LuaDocument {
                    root: xml::LuaElement(root.clone()),
                })?,
            )?;
        }

        lua.load(code)
            .set_name(filename)
            .set_mode(mlua::ChunkMode::Text)
            .set_environment(env)
            .exec()?;

        if let Some(root) = luatree {
            // SAFETY: The arena's DynamicRootSet will stay alive indefinitely
            context.document_root = Some(match dom::to_tree(unsize_node!(unsafe { *root.as_ptr() })) {
                xmltree::Node::Element(element) => element,
                _ => unreachable!(),
            });
        }

        if context.print_arena_stats {
            let mut gc = lua.app_data_mut::<LuaArena>().unwrap();
            println!("allocated bytes: {:?}", gc.metrics().total_allocation());
            println!("allocated bytes (gc only): {:?}", gc.metrics().total_gc_allocation());
            println!("debt: {:?}", gc.metrics().allocation_debt());
            gc.collect_all();
            println!(
                "allocated bytes after collection (gc only): {:?}",
                gc.metrics().total_gc_allocation()
            );
            println!(
                "allocated bytes after collection: {:?}",
                gc.metrics().total_allocation()
            );
        }

        Ok(())
    }
}
