use std::{collections::HashSet, mem::offset_of};

use crate::xmltree::{Element, Node};

type XMLNode = Node;

mod parse;
pub use parse::*;

// FIXME: This is a giant hack
const REMOVE_MARKER: &str = "_FTLMAN_INTERNAL_REMOVE_MARKER";

const MOD_NAMESPACES: &[&str] = &["mod", "mod-append", "mod-prepend", "mod-overwrite"];

pub enum PatchError<'s> {
    Panic(&'s FindPanic),
    AlreadyReported,
}

pub fn patch<'s>(context: &mut Element, script: &'s Script) -> Result<(), PatchError<'s>> {
    for node in &script.0 {
        match node {
            FindOrContent::Find(find) => {
                for element in mod_find(context, find)? {
                    mod_commands(element, &find.commands)?;
                }
            }
            FindOrContent::Content(node) => {
                let mut new = node.clone();
                if let Some(e) = new.as_mut_element() {
                    cleanup(e)
                }
                context.children.push(new);
            }
            FindOrContent::Error => return Err(PatchError::AlreadyReported),
        }
    }

    cleanup(context);

    Ok(())
}

trait ElementFilter {
    fn filter(&self, element: &Element) -> bool;
}

impl ElementFilter for SelectorFilter {
    fn filter(&self, element: &Element) -> bool {
        if self.name.as_ref().is_some_and(|filter| !filter.is_match(&element.name)) {
            return false;
        }

        for (key, value) in self.attrs.iter() {
            if element.attributes.get(key).is_none_or(|x| !value.is_match(x)) {
                return false;
            }
        }

        if self
            .value
            .as_ref()
            .is_some_and(|value| !value.is_match(&element.get_text_trim()))
        {
            return false;
        }

        true
    }
}

impl ElementFilter for WithChildFilter {
    fn filter(&self, element: &Element) -> bool {
        if self.name.as_ref().is_some_and(|filter| !filter.is_match(&element.name)) {
            return false;
        }

        element
            .children
            .iter()
            .filter_map(XMLNode::as_element)
            .any(|child| self.child_filter.filter(child))
    }
}

impl ElementFilter for SimpleFilter {
    fn filter(&self, element: &Element) -> bool {
        match self {
            SimpleFilter::Selector(filter) => filter.filter(element),
            SimpleFilter::WithChild(filter) => filter.filter(element),
        }
    }
}

fn cleanup(element: &mut Element) {
    if element.prefix.as_deref().is_some_and(|x| MOD_NAMESPACES.contains(&x)) {
        element.prefix = None
    }

    for child in std::mem::take(&mut element.children) {
        match child {
            XMLNode::Element(e) if e.prefix.as_deref() == Some(REMOVE_MARKER) => {}
            XMLNode::Element(mut e) => {
                cleanup(&mut e);
                element.children.push(XMLNode::Element(e))
            }
            XMLNode::Comment(..) => {}
            n => element.children.push(n),
        }
    }
}

/// # Safety
///
/// All pointers in `elements` must be immutably borrowable.
unsafe fn mod_find_raw<'s>(
    elements: &[*mut Element],
    result: &mut Vec<*mut Element>,
    find: &'s Find,
) -> Result<(), PatchError<'s>> {
    debug_assert!(result.is_empty());

    match &find.filter {
        FindFilter::Simple(filter) => result.extend(
            elements
                .iter()
                .copied()
                .filter(|&element| unsafe { filter.filter(&*element) }),
        ),
        FindFilter::Composite(filter) => {
            let mut it = filter.filters.iter();

            let Some(first) = it.next() else {
                if filter.operation.complement {
                    result.extend_from_slice(elements);
                } else {
                    // leave result empty
                }

                return Ok(());
            };

            unsafe { mod_find_raw(elements, result, first)? };
            let mut set: HashSet<*mut Element> = result.drain(..).collect();

            for child in it {
                unsafe { mod_find_raw(elements, result, child)? };

                match filter.operation.operator {
                    ParOperator::And => {
                        let candidate_set = result.drain(..).collect::<HashSet<_>>();
                        set.retain(|x| candidate_set.contains(x));
                    }
                    ParOperator::Or => set.extend(result.drain(..)),
                }
            }

            if filter.operation.complement {
                result.extend(elements.iter().filter(|&c| !set.contains(c)));
            } else {
                result.extend(set);
                result.sort_unstable();
            }
        }
    };

    if find.reverse {
        result.reverse();
    }

    result.truncate(find.start + find.limit);
    result.drain(..find.start.min(result.len()));

    if let Some(panic_location) = find.panic.as_ref().filter(|_| result.is_empty()) {
        return Err(PatchError::Panic(panic_location));
    }

    Ok(())
}

fn mod_find<'a, 's>(context: &'a mut Element, find: &'s Find) -> Result<Vec<&'a mut Element>, PatchError<'s>> {
    let elements = context
        .children
        .iter_mut()
        .filter_map(Node::as_mut_element)
        // Previously removed elements should not be findable again.
        .filter(|e| e.prefix.as_deref().is_none_or(|p| p != REMOVE_MARKER))
        .map(|e| e as *mut Element)
        .collect::<Vec<_>>();
    let mut result = Vec::new();
    unsafe { mod_find_raw(&elements, &mut result, find)? };
    Ok(result.into_iter().map(|x| unsafe { &mut *x }).collect())
}

fn mod_commands<'s>(context: &mut Element, commands: &'s [Command]) -> Result<(), PatchError<'s>> {
    for command in commands {
        match command {
            Command::Find(find) => {
                for matched in mod_find(context, find)? {
                    mod_commands(matched, &find.commands)?;
                }
            }
            Command::SetAttributes(attributes) => {
                context.attributes.extend(attributes.iter().cloned());
            }
            Command::RemoveAttributes(keys) => {
                for key in keys {
                    let _ = context.attributes.remove(key);
                }
            }
            Command::SetValue(value) => {
                context.children.clear();
                context.children.push(XMLNode::Text(value.to_string()))
            }
            Command::RemoveTag => context.prefix = Some(REMOVE_MARKER.into()),
            Command::Prepend(element) => {
                let mut new = element.clone();
                new.prefix = None;

                context.children.insert(0, XMLNode::Element(new));
            }
            Command::Append(element) => {
                let mut new = element.clone();
                new.prefix = None;

                context.children.push(XMLNode::Element(new));
            }
            Command::Overwrite(element) => {
                let mut new = element.clone();
                new.prefix = None;

                if let Some(old) = context.get_mut_child(&new.name) {
                    let _ = std::mem::replace(old, new);
                } else {
                    context.children.push(XMLNode::Element(new));
                }
            }
            Command::InsertByFind(command) => {
                let mut results = mod_find(context, &command.find)?;

                let before_iter = command.before.iter().map(|element| Node::Element(element.clone()));
                let after_iter = command.after.iter().map(|element| Node::Element(element.clone()));
                if results.is_empty() {
                    if command.add_anyway {
                        context.children.splice(0..0, before_iter);
                        context.children.splice(context.children.len().., after_iter);
                    }
                } else {
                    // NOTE: This whole "process" is kinda ""hacky"" but is a pretty efficient way to do this I think.

                    macro_rules! node_addr {
                        ($ref_to_mut_ref: expr) => {
                            // This will reverse-map an Element pointer that's part of a Node enum
                            // back to the address of the original Node.
                            // Requires unstable `offset_of_enum` feature.
                            ($ref_to_mut_ref as *mut Element)
                                .wrapping_byte_sub(offset_of!(Node, Element.0))
                                .addr()
                        };
                    }

                    let first_addr = node_addr!(*results.first_mut().unwrap());
                    let last_addr = node_addr!(*results.last_mut().unwrap());
                    drop(results);

                    let start_addr = context.children.as_ptr().addr();

                    let first_idx = (first_addr - start_addr) / std::mem::size_of::<Node>();
                    let last_idx = (last_addr - start_addr) / std::mem::size_of::<Node>();

                    let before_len = command.before.len();
                    context.children.splice(first_idx..first_idx, before_iter);
                    let after_insert_idx = last_idx + before_len + 1;
                    context.children.splice(after_insert_idx..after_insert_idx, after_iter);
                }
            }
            Command::Error => return Err(PatchError::AlreadyReported),
        }
    }

    Ok(())
}
