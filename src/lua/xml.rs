use std::fmt::Write as _;

use gc_arena::{
    lock::{GcRefLock, RefLock},
    DynamicRoot, DynamicRootSet, Gc, Mutation, Rootable,
};
use mlua::{prelude::*, FromLua, UserData, UserDataFields};

use crate::xmltree::dom::{self, ElementChildren, GcElement, GcNode, GcText, NodeExt, NodeTraits};

use super::{unsize_node, LuaExt};

type DynamicNode = DynamicRoot<Rootable![GcNode<'_>]>;
type DynamicElement = DynamicRoot<Rootable![GcElement<'_>]>;

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
            Ok(unsafe { this.get_node() }
                .borrow()
                .previous_sibling()
                .map(|value| roots.stash(mc, value))
                .map(DynamicIntoLua))
        })
    });
    fields.add_field_method_get("nextSibling", |lua, this| {
        lua.gc().mutate(|mc, roots| {
            Ok(unsafe { this.get_node() }
                .borrow()
                .next_sibling()
                .map(|value| roots.stash(mc, value))
                .map(DynamicIntoLua))
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

    // methods.add_method("before", |_, this, nodes: mlua::Variadic<NodeImplicitlyConvertible>| {
    //     for node in nodes.into_iter().rev() {
    //         this.as_node_mut()
    //     }
    // });
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

#[repr(transparent)]
struct DynamicIntoLua(pub DynamicNode);

fn gc_into_lua<'gc>(
    mc: &Mutation<'gc>,
    roots: &DynamicRootSet<'gc>,
    lua: &Lua,
    node: GcNode<'gc>,
) -> LuaResult<LuaValue> {
    let kind = node.borrow().kind();

    match kind {
        dom::NodeKind::Element => {
            LuaElement(roots.stash(mc, unsafe { dom::Element::downcast_gc_unchecked(node) })).into_lua(lua)
        }
        dom::NodeKind::Comment => todo!(),
        dom::NodeKind::CData => todo!(),
        dom::NodeKind::Text => {
            LuaText(roots.stash(mc, unsafe { dom::Text::downcast_gc_unchecked(node) })).into_lua(lua)
        }
        dom::NodeKind::ProcessingInstruction => todo!(),
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
        dom::NodeKind::ProcessingInstruction => downcast_and_detach!(dom::ProcessingInstruction),
    }
}

impl IntoLua for DynamicIntoLua {
    fn into_lua<'gc>(self, lua: &Lua) -> LuaResult<LuaValue> {
        lua.gc().mutate(|mc, roots| {
            let node = *roots.fetch(&self.0);
            gc_into_lua(mc, roots, lua, node)
        })
    }
}

#[derive(Clone)]
enum NodeImplicitlyConvertible {
    Element(LuaElement),
    String(String),
}

impl FromLua for NodeImplicitlyConvertible {
    fn from_lua(value: mlua::Value, lua: &mlua::Lua) -> mlua::Result<Self> {
        let type_name = value.type_name();
        match LuaElement::from_lua(value.clone(), lua) {
            Ok(element) => Ok(Self::Element(element)),
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

impl NodeImplicitlyConvertible {
    fn into_node<'gc>(self, mc: &Mutation<'gc>) -> GcNode<'gc> {
        match self {
            NodeImplicitlyConvertible::Element(e) => unsize_node!(unsafe { *e.0.as_ptr() }),
            NodeImplicitlyConvertible::String(text) => unsize_node!(dom::Text::create(mc, text)),
        }
    }
}

impl UserData for LuaElement {
    fn add_fields<F: UserDataFields<Self>>(fields: &mut F) {
        add_node_fields(fields, "element");

        fields.add_field_method_get("name", |_, this| Ok(unsafe { *this.0.as_ptr() }.borrow().name.clone()));
        fields.add_field_method_set("name", |_, this, value: String| {
            // SAFETY: No write barrier has to be triggered as no Gc pointers are modified.
            unsafe { (*this.0.as_ptr()).as_ref_cell() }.borrow_mut().name = value;
            Ok(())
        });
        fields.add_field_method_get("prefix", |_, this| {
            Ok(unsafe { *this.0.as_ptr() }.borrow().prefix.clone())
        });
        fields.add_field_method_set("prefix", |_, this, value: Option<String>| {
            // SAFETY: See above
            unsafe { (*this.0.as_ptr()).as_ref_cell() }.borrow_mut().prefix = value;
            Ok(())
        });

        fields.add_field_method_get("textContent", |_, this| {
            let this = unsafe { *this.0.as_ptr() };
            let mut output = String::new();
            for child in this.borrow().children() {
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
                    let mut this = roots.fetch(&this.0).borrow_mut(mc);
                    for (i, node) in nodes.into_iter().enumerate().rev() {
                        let node = node.into_node(mc);
                        if node.borrow().parent().is_some() {
                            return Err(LuaError::runtime(format!(
                                "Node passed as argument #{} to Element:prepend already has a parent",
                                i + 1
                            )));
                        }
                        this.prepend_child(mc, node);
                    }
                    Ok(())
                })
            },
        );
        methods.add_method(
            "append",
            |lua, this, nodes: mlua::Variadic<NodeImplicitlyConvertible>| {
                lua.gc().mutate(|mc, roots| {
                    let mut this = roots.fetch(&this.0).borrow_mut(mc);
                    for (i, node) in nodes.into_iter().enumerate() {
                        let node = node.into_node(mc);
                        if node.borrow().parent().is_some() {
                            return Err(LuaError::runtime(format!(
                                "Node passed as argument #{} to Element:append already has a parent",
                                i + 1
                            )));
                        }
                        this.append_child(mc, node);
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

        methods.add_method("fistChild", |lua, this, _: ()| {
            lua.gc().mutate(|mc, roots| {
                roots
                    .fetch(&this.0)
                    .borrow()
                    .children()
                    .next()
                    .map(|node| gc_into_lua(mc, roots, lua, node))
                    .transpose()
            })
        });

        methods.add_method("fistChild", |lua, this, _: ()| {
            lua.gc().mutate(|mc, roots| {
                roots
                    .fetch(&this.0)
                    .borrow()
                    .children()
                    .next_back()
                    .map(|node| gc_into_lua(mc, roots, lua, node))
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
            element_tostring(unsafe { &(*this.0.as_ptr()).borrow() }, &mut output);
            Ok(output)
        });

        #[cfg(debug_assertions)]
        methods.add_method("debug", |_, this, _: ()| {
            let mut output = String::new();
            write!(output, "{:#?}", unsafe { *this.0.as_ptr() }.borrow()).unwrap();
            Ok(output)
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
                    iter.next().map(|node| gc_into_lua(mc, roots, lua, node))
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

            Ok(LuaElement(lua.gc().mutate(|mc, roots| {
                roots.stash(
                    mc,
                    dom::Element::create(mc, prefix, name, Option::unwrap_or_default(attributes)),
                )
            })))
        })?,
    )?;

    Ok(table)
}
