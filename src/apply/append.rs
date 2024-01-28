use std::{borrow::Cow, collections::HashMap};

use anyhow::{anyhow, bail, Result};
use xmltree::{Element, XMLNode};

// FIXME: This is a giant hack
const REMOVE_MARKER: &str = "_FTLMAN_INTERNAL_REMOVE_MARKER";

pub fn patch(context: &mut Element, patch: Element) -> Result<()> {
    for mut node in patch.children {
        match node {
            XMLNode::Element(el) if el.namespace.as_deref() == Some("mod") => {
                let Some(matches) = mod_find(context, &el)? else {
                    bail!("Unrecognised mod tag {}", el.name);
                };

                for element in matches {
                    mod_commands(element, &el)?;
                }
            }
            XMLNode::Comment(..) => (),
            _ => {
                if let Some(e) = node.as_mut_element() {
                    cleanup(e)
                }
                context.children.push(node);
            }
        }
    }

    cleanup(context);

    Ok(())
}

fn cleanup(element: &mut Element) {
    if element.namespace.as_deref() == Some("mod") {
        element.namespace = None
    }

    if let Some(ns) = element.namespaces.as_mut() {
        ns.0.remove("mod");
        ns.0.remove("mod-append");
        ns.0.remove("mod-overwrite");
    }

    for (k, v) in std::mem::take(&mut element.attributes) {
        if !k.starts_with("xmlns:mod") {
            element.attributes.insert(k, v);
        }
    }

    for child in std::mem::take(&mut element.children) {
        match child {
            XMLNode::Element(e) if e.namespace.as_deref() == Some(REMOVE_MARKER) => {}
            XMLNode::Element(mut e) => {
                cleanup(&mut e);
                element.children.push(XMLNode::Element(e))
            }
            XMLNode::Comment(..) => {}
            n => element.children.push(n),
        }
    }
}

// TODO: ModFindError
fn mod_find<'a>(context: &'a mut Element, node: &Element) -> Result<Option<Vec<&'a mut Element>>> {
    if node.namespace.as_ref().is_some_and(|x| x == "mod") {
        macro_rules! get_attr {
            ($tag: literal, $type: ty, $name: literal, $default: expr) => {{
                get_attr!($tag, $type, $name)
                    .transpose()
                    .unwrap_or(Ok($default))
            }};
            ($tag: literal, $type: ty, $name: literal) => {{
                node.attributes
                    .get($name)
                    .map(|text| {
                        text.parse::<$type>().map_err(|_| {
                            anyhow!(concat!(
                                "mod:",
                                $tag,
                                " ",
                                $name,
                                "attribute has invalid value {v}"
                            ))
                        })
                    })
                    .transpose()
            }};
        }

        match node.name.as_str() {
            "findName" => {
                let Some(search_name) = get_attr!("findName", String, "name")? else {
                    bail!("findName requires a name attribute");
                };
                let search_type = get_attr!("findName", String, "type")?;
                let search_reverse = get_attr!("findName", bool, "reverse", true)?;
                let search_start = get_attr!("findName", usize, "start", 0)?;
                let search_limit = get_attr!("findName", isize, "limit", 1)?;
                let panic = get_attr!("findName", bool, "panic", false)?;

                if search_type.is_some_and(|x| x.is_empty()) {
                    bail!("findName 'type' attribute cannot be empty")
                }

                if search_limit < -1 {
                    bail!("findName 'limit' attribute must be >= -1")
                }

                let mut matches = vec![];

                for child in context.children.iter_mut() {
                    if let XMLNode::Element(ref mut element) = child {
                        if element
                            .attributes
                            .get("name")
                            .is_some_and(|x| x == &search_name)
                        {
                            matches.push(element);
                        }
                    }
                }

                let it = if search_reverse {
                    Box::new(matches.into_iter().rev()) as Box<dyn Iterator<Item = &mut Element>>
                } else {
                    Box::new(matches.into_iter()) as Box<dyn Iterator<Item = &mut Element>>
                };

                matches = it
                    .skip(search_start)
                    .take(if search_limit == -1 {
                        usize::MAX
                    } else {
                        search_limit as usize
                    })
                    .collect();

                Ok(Some(matches))
            }
            "findLike" => {
                let search_type = get_attr!("findName", String, "type")?;
                let search_reverse = get_attr!("findName", bool, "reverse", false)?;
                let search_start = get_attr!("findName", usize, "start", 0)?;
                let search_limit = get_attr!("findName", isize, "limit", -1)?;
                let panic = get_attr!("findName", bool, "panic", false)?;

                if search_type.as_ref().is_some_and(|x| x.is_empty()) {
                    bail!("findName 'type' attribute cannot be empty")
                }

                if search_limit < -1 {
                    bail!("findName 'limit' attribute must be >= -1")
                }

                let mut attrs = HashMap::new();
                let mut search_text = None;

                if let Some(selector) = node.get_child(("selector".to_string(), "mod".to_string()))
                {
                    for (key, value) in &selector.attributes {
                        if value.is_empty() {
                            bail!("selector attributes cannot be empty");
                        }

                        attrs.insert(key.to_owned(), value.to_owned());
                    }

                    let text = selector.get_text().unwrap_or(Cow::Borrowed(""));
                    search_text = if text.is_empty() {
                        None
                    } else {
                        Some(text.trim().to_string())
                    };
                }

                let mut matches = vec![];

                for child in context.children.iter_mut() {
                    if let XMLNode::Element(ref mut element) = child {
                        if let Some(ref tp) = search_type {
                            if &node.name != tp {
                                continue;
                            }
                        }

                        for (key, value) in &attrs {
                            if node.attributes.get(key) != Some(value) {
                                continue;
                            }
                        }

                        if let Some(ref text) = search_text {
                            if element.get_text().unwrap_or(Cow::Borrowed("")).trim() != text {
                                continue;
                            }
                        }

                        matches.push(element);
                    }
                }

                let it = if search_reverse {
                    Box::new(matches.into_iter().rev()) as Box<dyn Iterator<Item = &mut Element>>
                } else {
                    Box::new(matches.into_iter()) as Box<dyn Iterator<Item = &mut Element>>
                };

                matches = it
                    .skip(search_start)
                    .take(if search_limit == -1 {
                        usize::MAX
                    } else {
                        search_limit as usize
                    })
                    .collect();

                Ok(Some(matches))
            }
            _ => Ok(None),
        }
    } else {
        Ok(None)
    }
}

fn mod_commands(context: &mut Element, element: &Element) -> Result<()> {
    for command_node in &element.children {
        if let XMLNode::Element(command_element) = command_node {
            match command_element.namespace.as_deref() {
                Some("mod") => {
                    if let Some(matches) = mod_find(context, command_element)? {
                        for element in matches {
                            mod_commands(element, command_element)?;
                        }
                    } else {
                        match command_element.name.as_str() {
                            "selector" | "par" => {}
                            "setAttributes" => {
                                context.attributes.extend(
                                    command_element
                                        .attributes
                                        .iter()
                                        .map(|(k, v)| (k.to_owned(), v.to_owned())),
                                );
                            }
                            "removeAttributes" => {
                                for key in command_element.attributes.keys() {
                                    let _ = context.attributes.remove(key);
                                }
                            }
                            "setValue" => {
                                // Remove all text and cdata nodes
                                for node in std::mem::take(&mut context.children) {
                                    match node {
                                        XMLNode::Element(_)
                                        | XMLNode::Comment(_)
                                        | XMLNode::ProcessingInstruction(_, _) => {
                                            context.children.push(node)
                                        }
                                        XMLNode::CData(_) | XMLNode::Text(_) => {}
                                    }
                                }

                                context.children.push(XMLNode::Text(
                                    command_element
                                        .get_text()
                                        .map(|x| x.trim().to_string())
                                        .unwrap_or_else(String::new),
                                ))
                            }
                            "removeTag" => {
                                context.namespace = Some(REMOVE_MARKER.to_string());
                            }
                            _ => {
                                bail!("Unrecognised mod tag {}", command_element.name)
                            }
                        }
                    }
                }
                Some("mod-append") => {
                    let mut new = element.clone();
                    new.namespace = None;
                    context.children.push(XMLNode::Element(new));
                }
                Some("mod-overwrite") => {
                    let mut new = element.clone();
                    new.namespace = None;

                    if let Some(old) = context.get_mut_child(new.name.as_str()) {
                        let _ = std::mem::replace(old, new);
                    } else {
                        context.children.push(XMLNode::Element(new));
                    }
                }
                _ => bail!("Unrecognised mod command tag {}", command_element.name),
            }
        }
    }

    Ok(())
}
