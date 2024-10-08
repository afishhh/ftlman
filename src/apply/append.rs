use std::{
    collections::{HashMap, HashSet},
    fmt::Write,
    str::FromStr,
};

use crate::xmltree::{Element, Node};
use anyhow::{anyhow, bail, Result};
use lazy_static::lazy_static;
use regex::Regex;

type XMLNode = Node;

// FIXME: This is a giant hack
const REMOVE_MARKER: &str = "_FTLMAN_INTERNAL_REMOVE_MARKER";

lazy_static! {
    static ref XML_COMMENT_REGEX: Regex = Regex::new("<!--(?s:.*?)-->").unwrap();
}

pub fn patch(context: &mut Element, patch: Vec<XMLNode>) -> Result<()> {
    for mut node in patch {
        match node {
            XMLNode::Element(el) if el.prefix.as_deref() == Some("mod") => {
                let Some(matches) = mod_find(context, &el)? else {
                    bail!("Unrecognised mod find tag {}", el.name);
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
        .prefix
        .as_ref()
        .is_some_and(|x| MOD_NAMESPACES.contains(&x.as_str()))
    {
        element.prefix = None
    }

    if element
        .prefix
        .as_ref()
        .is_some_and(|x| MOD_NAMESPACES.contains(&x.as_str()))
    {
        element.prefix = None
    }

    if let Some(ns) = element.prefix.as_ref() {
        if MOD_NAMESPACES.contains(&ns.as_str()) {
            element.prefix = None;
        }
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

trait ElementFilter {
    fn filter_one(&self, element: &Element) -> bool;

    fn filter_children<'a>(&self, context: &'a mut Element) -> Vec<&'a mut Element> {
        context
            .children
            .iter_mut()
            .filter_map(|x| x.as_mut_element().filter(|child| self.filter_one(child)))
            .collect()
    }
}

#[derive(Default)]
struct SelectorFilter {
    pub name: Option<String>,
    pub attrs: Vec<(String, String)>,
    pub value: Option<String>,
}

impl SelectorFilter {
    fn from_selector_element(selector: &Element) -> Self {
        let mut result = Self {
            ..Default::default()
        };

        for (key, value) in &selector.attributes {
            result.attrs.push((key.to_owned(), value.to_owned()));
        }

        let text = selector.get_text();
        let trimmed = text.trim();
        if !trimmed.is_empty() {
            result.value = Some(trimmed.to_string())
        }

        result
    }

    fn from_selector_parent(parent: &Element) -> Self {
        parent
            .get_child(("selector", "mod"))
            .map(Self::from_selector_element)
            .unwrap_or_default()
    }
}

impl ElementFilter for SelectorFilter {
    fn filter_one(&self, element: &Element) -> bool {
        if self
            .name
            .as_deref()
            .is_some_and(|name| element.name != name)
        {
            return false;
        }

        for (key, value) in self.attrs.iter() {
            if element.attributes.get(key) != Some(value) {
                return false;
            }
        }

        if self
            .value
            .as_deref()
            .is_some_and(|value| value != element.get_text().trim())
        {
            return false;
        }

        true
    }
}

#[derive(Default)]
struct WithChildFilter<F: ElementFilter> {
    pub name: Option<String>,
    pub child_filter: F,
}

impl<F: ElementFilter> ElementFilter for WithChildFilter<F> {
    fn filter_one(&self, element: &Element) -> bool {
        if self
            .name
            .as_deref()
            .is_some_and(|name| element.name != name)
        {
            return false;
        }

        element
            .children
            .iter()
            .filter_map(XMLNode::as_element)
            .any(|child| self.child_filter.filter_one(child))
    }
}

fn index_children(node: &Element) -> HashMap<*const Element, usize> {
    let mut result = HashMap::new();

    for (i, child) in node
        .children
        .iter()
        .filter_map(XMLNode::as_element)
        .enumerate()
    {
        result.insert(child as *const Element, i);
    }

    result
}

fn mod_par<'a>(context: &'a mut Element, node: &Element) -> Result<Option<Vec<&'a mut Element>>> {
    if node.name != "par" || node.prefix.as_deref() != Some("mod") {
        return Ok(None);
    }

    let operation = get_attr!(node, ParOperation, "op")?
        .ok_or_else(|| anyhow!("par node is missing an op attribute"))?;

    let mut set = HashSet::<*mut Element>::new();

    for child in node.children.iter().filter_map(XMLNode::as_element) {
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
    if node.prefix.as_ref().is_some_and(|x| x == "mod") {
        if !["findName", "findLike", "findWithChildLike", "findComposite"]
            .contains(&node.name.as_str())
        {
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

                SelectorFilter {
                    name: search_type,
                    attrs: vec![("name".to_string(), search_name)],
                    value: None,
                }
                .filter_children(context)
            }
            "findLike" => {
                let search_type = get_attr!(node, String, "type")?;

                if search_type.as_ref().is_some_and(|x| x.is_empty()) {
                    bail!("findLike 'type' attribute cannot be empty")
                }

                SelectorFilter {
                    name: search_type,
                    ..SelectorFilter::from_selector_parent(node)
                }
                .filter_children(context)
            }
            "findWithChildLike" => {
                let search_type = get_attr!(node, String, "type")?;

                if search_type.as_ref().is_some_and(|x| x.is_empty()) {
                    bail!("findWithChildLike 'type' attribute cannot be empty")
                }

                let search_child_type = get_attr!(node, String, "child-type")?;

                if search_child_type.as_ref().is_some_and(|x| x.is_empty()) {
                    bail!("findWithChildLike 'child-type' attribute cannot be empty")
                }

                WithChildFilter {
                    name: search_type,
                    child_filter: SelectorFilter {
                        name: search_child_type,
                        ..SelectorFilter::from_selector_parent(node)
                    },
                }
                .filter_children(context)
            }
            "findComposite" => {
                let Some(par) = node.get_child(("par", "mod")) else {
                    bail!("findComposite element is missing a par child");
                };

                let index = index_children(context);
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
        match command.prefix.as_deref() {
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

                            context
                                .children
                                .push(XMLNode::Text(command.get_text().trim().to_string()))
                        }
                        "removeTag" => {
                            context.prefix = Some(REMOVE_MARKER.to_string());
                        }
                        _ => {
                            bail!("Unrecognised mod command tag {}", command.name)
                        }
                    }
                }
            }
            Some("mod-append") => {
                let mut new = command.clone();
                new.prefix = None;

                context.children.push(XMLNode::Element(new));
            }
            Some("mod-overwrite") => {
                let mut new = command.clone();
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
