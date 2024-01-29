use std::{
    borrow::Cow,
    collections::{HashMap, HashSet},
    fmt::Write,
    str::FromStr,
};

use anyhow::{anyhow, bail, Result};
use lazy_static::lazy_static;
use regex::Regex;
use xmltree::{Element, XMLNode};

// FIXME: This is a giant hack
const REMOVE_MARKER: &str = "_FTLMAN_INTERNAL_REMOVE_MARKER";

lazy_static! {
    static ref XML_COMMENT_REGEX: Regex = Regex::new("<!--(?s:.*?)-->").unwrap();
}

// Sometimes our beloved mods (Multiverse) contain comments in the form
// <!-- something something <!--  more things -->
// which are rejected by xml-rs.
// Since we don't care about comments we can just remove everything
// that even resembles a comment.
// Hopefully this doesn't break anything.
pub fn clean_xml(xml: &str) -> Cow<'_, str> {
    XML_COMMENT_REGEX.replace_all(xml, "")
}

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
    const MOD_NAMESPACES: &[&str] = &["mod", "mod-append", "mod-overwrite"];

    if element
        .namespace
        .as_ref()
        .is_some_and(|x| MOD_NAMESPACES.contains(&x.as_str()))
    {
        element.namespace = None
    }

    if element
        .prefix
        .as_ref()
        .is_some_and(|x| MOD_NAMESPACES.contains(&x.as_str()))
    {
        element.prefix = None
    }

    if let Some(ns) = element.namespaces.as_mut() {
        for mns in MOD_NAMESPACES {
            ns.0.remove(*mns);
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ParOperation {
    And,
    Or,
}

impl FromStr for ParOperation {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "AND" => Ok(ParOperation::And),
            "OR" => Ok(ParOperation::Or),
            _ => bail!("Invalid par operation: {s}"),
        }
    }
}

macro_rules! get_attr {
    ($node: ident, $type: ty, $name: literal, $default: expr) => {{
        get_attr!($node, $type, $name)
            .transpose()
            .unwrap_or(Ok($default))
    }};
    ($node: ident, $type: ty, $name: literal) => {{
        $node
            .attributes
            .get($name)
            .map(|text| {
                text.parse::<$type>().map_err(|_| {
                    anyhow!(
                        concat!("mod:{}", " attribute ", $name, " has invalid value {}"),
                        $node.name,
                        text
                    )
                })
            })
            .transpose()
    }};
}

// FIXME: This is terrible for performance, but should also be used rarely and on small subtress.
fn index_subtree(node: &Element) -> HashMap<*const Element, u64> {
    fn rec(out: &mut HashMap<*const Element, u64>, node: &Element, mut next: u64) {
        out.insert(node as *const Element, next);
        next += 1;

        for child in node.children.iter().filter_map(|x| x.as_element()) {
            rec(out, child, next);
            next += 1;
        }
    }

    let mut result = HashMap::new();
    rec(&mut result, node, 0);
    result
}

fn mod_par<'a>(context: &'a mut Element, node: &Element) -> Result<Option<Vec<&'a mut Element>>> {
    if node.name != "par" || node.namespace.as_deref() != Some("mod") {
        return Ok(None);
    }

    let operation = get_attr!(node, ParOperation, "op")?
        .ok_or_else(|| anyhow!("par node is missing an op attribute"))?;

    let mut set = HashSet::<*mut Element>::new();

    for child in node.children.iter().filter_map(|x| x.as_element()) {
        // FIXME: What did I just do
        let Some(candidates) = (match mod_par(context, child)? {
            Some(x) => Some(x),
            None => mod_find(context, child)?,
        }) else {
            bail!("par node contains an invalid child");
        };

        match operation {
            ParOperation::And => {
                let candidate_set = candidates
                    .into_iter()
                    .map(|x| x as *mut Element)
                    .collect::<HashSet<_>>();
                set.retain(|x| candidate_set.contains(x));
            }
            ParOperation::Or => {
                set.extend(candidates.into_iter().map(|x| x as *mut Element));
            }
        }
    }

    // SAFETY: This is a set so obviously all the pointers are going to be unique.
    //         There are no other pointers that can point to these elements apart from
    //         these.
    Ok(Some(set.into_iter().map(|x| unsafe { &mut *x }).collect()))
}

// FIXME: The code duplication here is actually atrocious
fn mod_find<'a>(context: &'a mut Element, node: &Element) -> Result<Option<Vec<&'a mut Element>>> {
    if node.namespace.as_ref().is_some_and(|x| x == "mod") {
        if !["findName", "findLike", "findComposite"].contains(&node.name.as_str()) {
            return Ok(None);
        }

        let search_reverse = get_attr!(node, bool, "reverse", true)?;
        let search_start = get_attr!(node, usize, "start", 0)?;
        let search_limit = get_attr!(
            node,
            isize,
            "limit",
            if node.name == "findName" { 1 } else { -1 }
        )?;

        if search_limit < -1 {
            bail!("{} 'limit' attribute must be >= -1", node.name)
        }

        let panic = get_attr!(node, bool, "panic", false)?;

        let mut matches: Vec<&'a mut Element> = match node.name.as_str() {
            "findName" => {
                let Some(search_name) = get_attr!(node, String, "name")? else {
                    bail!("findName requires a name attribute");
                };
                let search_type = get_attr!(node, String, "type")?;

                if search_type.as_ref().is_some_and(|x| x.is_empty()) {
                    bail!("findName 'type' attribute cannot be empty")
                }

                context
                    .children
                    .iter_mut()
                    .filter_map(|x| {
                        x.as_mut_element().filter(|element| {
                            element
                                .attributes
                                .get("name")
                                .is_some_and(|x| x == &search_name)
                                && search_type
                                    .as_ref()
                                    .map(|x| element.name == *x)
                                    .unwrap_or(true)
                        })
                    })
                    .collect()
            }
            "findLike" => {
                let search_type = get_attr!(node, String, "type")?;

                if search_type.as_ref().is_some_and(|x| x.is_empty()) {
                    bail!("findLike 'type' attribute cannot be empty")
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

                context
                    .children
                    .iter_mut()
                    .filter_map(|x| {
                        x.as_mut_element().filter(|element| {
                            if let Some(ref tp) = search_type {
                                if &node.name != tp {
                                    return false;
                                }
                            }

                            for (key, value) in &attrs {
                                if node.attributes.get(key) != Some(value) {
                                    return false;
                                }
                            }

                            if let Some(ref text) = search_text {
                                if element.get_text().unwrap_or(Cow::Borrowed("")).trim() != text {
                                    return false;
                                }
                            }

                            true
                        })
                    })
                    .collect()
            }
            "findComposite" => {
                let Some(par) = node.children.iter().find_map(|x| {
                    x.as_element()
                        .filter(|x| x.name == "par" && x.namespace.as_deref() == Some("mod"))
                }) else {
                    bail!("findComposite element is missing a par child");
                };

                let index = index_subtree(context);
                let mut vec = mod_par(context, par)?.unwrap();

                vec.sort_by_key(|x| index.get(&(*x as *const Element)).unwrap());

                vec
            }
            _ => unreachable!(),
        };

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

        if panic && matches.is_empty() {
            let mut msg = format!(
                "{} element has panic=true but no elements matched",
                node.name
            );

            for (k, v) in node.attributes.iter() {
                write!(msg, "\n\t{k}={v}").unwrap();
            }

            bail!("{msg}");
        }

        Ok(Some(matches))
    } else {
        Ok(None)
    }
}

fn mod_commands(context: &mut Element, element: &Element) -> Result<()> {
    for command in element.children.iter().filter_map(|x| x.as_element()) {
        match command.namespace.as_deref() {
            Some("mod") => {
                if let Some(matches) = mod_find(context, command)? {
                    for matched in matches {
                        mod_commands(matched, command)?;
                    }
                } else {
                    match command.name.as_str() {
                        "selector" | "par" => {}
                        "setAttributes" => {
                            context.attributes.extend(
                                command
                                    .attributes
                                    .iter()
                                    .map(|(k, v)| (k.to_owned(), v.to_owned())),
                            );
                        }
                        "removeAttributes" => {
                            for key in command.attributes.keys() {
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
                                command
                                    .get_text()
                                    .map(|x| x.trim().to_string())
                                    .unwrap_or_else(String::new),
                            ))
                        }
                        "removeTag" => {
                            context.namespace = Some(REMOVE_MARKER.to_string());
                        }
                        _ => {
                            bail!("Unrecognised mod tag {}", command.name)
                        }
                    }
                }
            }
            Some("mod-append") => {
                let mut new = command.clone();
                new.namespace = None;
                new.prefix = None;

                // println!("appending {:?} to {:?}", new, context);

                context.children.push(XMLNode::Element(new));
            }
            Some("mod-overwrite") => {
                let mut new = command.clone();
                new.namespace = None;
                new.prefix = None;

                if let Some(old) = context.get_mut_child(new.name.as_str()) {
                    let _ = std::mem::replace(old, new);
                } else {
                    context.children.push(XMLNode::Element(new));
                }
            }
            _ => bail!("Unrecognised mod command namespace {}", command.name),
        }
    }

    Ok(())
}
