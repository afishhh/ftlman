use std::{collections::HashSet, mem::offset_of};

use crate::xmltree::{Element, Node};

type XMLNode = Node;

mod parse;
pub use parse::*;

// FIXME: This is a giant hack
const REMOVE_MARKER: &str = "_FTLMAN_INTERNAL_REMOVE_MARKER";

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

    fn filter_children<'a>(&self, context: &'a mut Element) -> Vec<&'a mut Element> {
        context
            .children
            .iter_mut()
            .filter_map(|x| x.as_mut_element().filter(|child| self.filter(child)))
            .collect()
    }
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
    const MOD_NAMESPACES: &[&str] = &["mod", "mod-append", "mod-prepend", "mod-overwrite"];

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

fn mod_find<'a, 's>(context: &'a mut Element, find: &'s Find) -> Result<Vec<&'a mut Element>, PatchError<'s>> {
    let mut matches = match &find.filter {
        FindFilter::Simple(filter) => filter.filter_children(context),
        // PERF: This could theoretically be optimised to run in two passes instead of using a set.
        FindFilter::Composite(filter) => {
            let mut it = filter.filters.iter();

            let Some(first) = it.next() else {
                return Ok(if filter.operation.complement {
                    context.children.iter_mut().filter_map(Node::as_mut_element).collect()
                } else {
                    Vec::new()
                });
            };

            let mut set: HashSet<*mut Element> = mod_find(context, first)?
                .into_iter()
                .map(|e| e as *mut Element)
                .collect();

            for child in it {
                let candidates = mod_find(context, child)?;

                match filter.operation.operator {
                    ParOperator::And => {
                        let candidate_set = candidates
                            .into_iter()
                            .map(|x| x as *mut Element)
                            .collect::<HashSet<_>>();
                        set.retain(|x| candidate_set.contains(x));
                    }
                    ParOperator::Or => {
                        set.extend(candidates.into_iter().map(|x| x as *mut Element));
                    }
                }
            }

            // TODO: I believe these pointers are actually invalidated
            //       by the time we get here but I don't care enough to fix this
            //       right now.
            if filter.operation.complement {
                context
                    .children
                    .iter_mut()
                    .filter_map(Node::as_mut_element)
                    .map(|c| c as *mut Element)
                    .filter(|c| !set.contains(c))
                    .map(|c| unsafe { &mut *c })
                    .collect()
            } else {
                set.into_iter().map(|x| unsafe { &mut *x }).collect()
            }
        }
    };

    // TODO: Simplify this by iterating in reverse in filter_children
    let it = if find.reverse {
        Box::new(matches.into_iter().rev()) as Box<dyn Iterator<Item = &mut Element>>
    } else {
        Box::new(matches.into_iter()) as Box<dyn Iterator<Item = &mut Element>>
    };

    matches = it.skip(find.start).take(find.limit).collect();

    if let Some(panic_location) = find.panic.as_ref().filter(|_| matches.is_empty()) {
        return Err(PatchError::Panic(panic_location));
    }

    Ok(matches)
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
                context
                    .children
                    .retain(|node| !matches!(node, XMLNode::CData(_) | XMLNode::Text(_)));
                context.children.push(XMLNode::Text(value.to_string()))
            }
            Command::RemoveTag => context.prefix = Some(REMOVE_MARKER.into()),
            Command::Prepend(element) => {
                let mut new = element.clone();
                new.prefix = None;

                // FIXME: asymtotically ugly
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
                let results = mod_find(context, &command.find)?;

                let before_cloned_iter = command.before.iter().map(|element| Node::Element(element.clone()));
                let after_cloned_iter = command.after.iter().map(|element| Node::Element(element.clone()));
                if results.is_empty() {
                    if command.add_anyway {
                        context.children.splice(0..0, before_cloned_iter);
                        context.children.splice(context.children.len().., after_cloned_iter);
                    }
                } else {
                    // NOTE: This whole "process" is kinda ""hacky"" but is a pretty efficient way to do this I think.

                    macro_rules! unwrap_ptr {
                        ($ref_to_mut_ref: expr) => {
                            unsafe {
                                // This should be able to reverse-map an Element pointer that's part of a
                                // Node enum back to the address of the original Node.
                                // Requires unstable `offset_of_enum` feature.
                                (*($ref_to_mut_ref as *const _ as *const *mut Element))
                                    .byte_sub(offset_of!(Node, Element.0)) as *mut Node
                            }
                        };
                    }

                    let first = unwrap_ptr!(results.first().unwrap());
                    let last = unwrap_ptr!(results.last().unwrap());

                    debug_assert!(first.is_aligned() && last.is_aligned());
                    let range = context.children.as_mut_ptr_range();
                    debug_assert!(range.contains(&first));
                    debug_assert!(range.contains(&last));

                    // SAFETY: last and first should both point to the same allocation as `context.children`.
                    let first_idx = unsafe { first.offset_from(range.start) as usize };
                    let last_idx = unsafe { last.offset_from(range.start) as usize };

                    // FIXME: This insertion strategy is not optimal. (does it matter?)
                    let before_len = command.before.len();
                    context.children.splice(first_idx..first_idx, before_cloned_iter);
                    let after_insert_idx = last_idx + before_len + 1;
                    context
                        .children
                        .splice(after_insert_idx..after_insert_idx, after_cloned_iter);
                }
            }
            Command::Error => return Err(PatchError::AlreadyReported),
        }
    }

    Ok(())
}
