use std::{collections::HashSet, fmt::Write, mem::offset_of, str::FromStr};

use crate::xmltree::{Element, Node};
use anyhow::{anyhow, bail, Context, Result};
use regex::Regex;

type XMLNode = Node;

// FIXME: This is a giant hack
const REMOVE_MARKER: &str = "_FTLMAN_INTERNAL_REMOVE_MARKER";

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
    const MOD_NAMESPACES: &[&str] = &["mod", "mod-append", "mod-prepend", "mod-overwrite"];

    if element.prefix.as_deref().is_some_and(|x| MOD_NAMESPACES.contains(&x)) {
        element.prefix = None
    }

    if element.prefix.as_deref().is_some_and(|x| MOD_NAMESPACES.contains(&x)) {
        element.prefix = None
    }

    if let Some(ns) = element.prefix.as_deref() {
        if MOD_NAMESPACES.contains(&ns) {
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
enum ParOperator {
    And,
    Or,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ParOperation {
    complement: bool,
    operator: ParOperator,
}

impl FromStr for ParOperation {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "AND" => Ok(ParOperation {
                complement: false,
                operator: ParOperator::And,
            }),
            "OR" => Ok(ParOperation {
                complement: false,
                operator: ParOperator::Or,
            }),
            "NOR" => Ok(ParOperation {
                complement: true,
                operator: ParOperator::Or,
            }),
            "NAND" => Ok(ParOperation {
                complement: true,
                operator: ParOperator::And,
            }),
            _ => bail!("Invalid par operation: {s}"),
        }
    }
}

#[repr(transparent)]
struct BoxFromStr(Box<str>);

impl FromStr for BoxFromStr {
    type Err = std::convert::Infallible;
    fn from_str(s: &str) -> std::result::Result<Self, Self::Err> {
        Ok(Self(s.into()))
    }
}

macro_rules! get_attr {
    ($node: ident, $type: ty, $name: literal, $default: expr) => {{
        get_attr!($node, $type, $name).transpose().unwrap_or(Ok($default))
    }};
    ($node: ident, StringFilter($is_regex: expr), $name: literal) => {
        // FIXME: Hope LLVM optimises the maps away? Is that far fetched?
        //        If it doesn't the wrapping can be moved into the inner macro.
        if $is_regex {
            get_attr!($node, Regex, $name).map(|x| x.map(StringFilter::Regex))
        } else {
            get_attr!($node, BoxFromStr, $name).map(|x| x.map(|x| StringFilter::Fixed(x.0)))
        }
    };
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

enum StringFilter {
    Fixed(Box<str>),
    Regex(Regex),
}

impl StringFilter {
    fn parse(pattern: &str, is_regex: bool) -> Result<Self> {
        Ok(if is_regex {
            Self::Regex(pattern.parse()?)
        } else {
            Self::Fixed(pattern.into())
        })
    }

    fn is_match(&self, value: &str) -> bool {
        match self {
            StringFilter::Fixed(text) => value == &**text,
            StringFilter::Regex(regex) => regex.find(value).is_some_and(|m| m.len() == value.len()),
        }
    }
}

#[derive(Default)]
struct SelectorFilter {
    pub name: Option<StringFilter>,
    pub attrs: Vec<(Box<str>, StringFilter)>,
    pub value: Option<StringFilter>,
}

impl SelectorFilter {
    fn from_selector_element(selector: &Element, regex: bool) -> Result<Self> {
        let mut result = Self { ..Default::default() };

        for (key, value) in &selector.attributes {
            result.attrs.push((key.to_owned(), StringFilter::parse(value, regex)?));
        }

        let text = selector.get_text_trim();
        if !text.is_empty() {
            result.value = Some(StringFilter::parse(&text, regex)?)
        }

        Ok(result)
    }

    fn from_selector_parent(parent: &Element, regex: bool) -> Result<Self> {
        if let Some(child) = parent.get_child(("selector", "mod")) {
            Self::from_selector_element(child, regex)
        } else {
            Ok(Self::default())
        }
    }
}

impl ElementFilter for SelectorFilter {
    fn filter_one(&self, element: &Element) -> bool {
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

#[derive(Default)]
struct WithChildFilter<F: ElementFilter> {
    pub name: Option<StringFilter>,
    pub child_filter: F,
}

impl<F: ElementFilter> ElementFilter for WithChildFilter<F> {
    fn filter_one(&self, element: &Element) -> bool {
        if self.name.as_ref().is_some_and(|filter| !filter.is_match(&element.name)) {
            return false;
        }

        element
            .children
            .iter()
            .filter_map(XMLNode::as_element)
            .any(|child| self.child_filter.filter_one(child))
    }
}

fn mod_par<'a>(context: &'a mut Element, node: &Element) -> Result<Option<Vec<&'a mut Element>>> {
    if &*node.name != "par" || node.prefix.as_deref() != Some("mod") {
        return Ok(None);
    }

    let operation =
        get_attr!(node, ParOperation, "op")?.ok_or_else(|| anyhow!("par node is missing an op attribute"))?;

    let mut it = node.children.iter().filter_map(XMLNode::as_element);

    let Some(first_child) = it.next() else {
        return Ok(Some(Vec::new()));
    };

    let Some(initial) = (match mod_par(context, first_child)? {
        Some(x) => Some(x),
        None => mod_find(context, first_child)?,
    }) else {
        bail!("par node contains an invalid child");
    };

    let mut set: HashSet<*mut Element> = initial.into_iter().map(|x| x as *mut Element).collect();

    for child in it {
        let Some(candidates) = (match mod_par(context, child)? {
            Some(x) => Some(x),
            None => mod_find(context, child)?,
        }) else {
            bail!("par node contains an invalid child");
        };

        match operation.operator {
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

    Ok(Some(if operation.complement {
        context
            .children
            .iter_mut()
            .filter_map(Node::as_mut_element)
            .map(|c| c as *mut Element)
            .filter(|c| !set.contains(c))
            // SAFETY: These were just created from a vector so they're all unique
            //         and no existing mutable references to them can exist.
            .map(|c| unsafe { &mut *c })
            .collect()
    } else {
        // SAFETY: `set` is a set so obviously all the pointers are going to be unique.
        //         There are no existing mutable references that can point to these elements.
        set.into_iter().map(|x| unsafe { &mut *x }).collect()
    }))
}

fn mod_find<'a>(context: &'a mut Element, node: &Element) -> Result<Option<Vec<&'a mut Element>>> {
    if node.prefix.as_deref().is_some_and(|x| x == "mod") {
        if !["findName", "findLike", "findWithChildLike", "findComposite"].contains(&&*node.name) {
            return Ok(None);
        }

        let search_reverse = get_attr!(node, bool, "reverse", &*node.name == "findName")?;
        let search_start = get_attr!(node, usize, "start", 0)?;
        let search_limit = get_attr!(node, isize, "limit", if &*node.name == "findName" { 1 } else { -1 })?;

        if search_limit < -1 {
            bail!("{} 'limit' attribute must be >= -1", node.name)
        }

        let panic = get_attr!(node, bool, "panic", false)?;

        let mut matches: Vec<&'a mut Element> = match &*node.name {
            "findName" => {
                let search_regex = get_attr!(node, bool, "regex", false)?;

                let Some(search_name) = get_attr!(node, StringFilter(search_regex), "name")? else {
                    bail!("findName requires a name attribute");
                };

                let search_type = get_attr!(node, StringFilter(search_regex), "type")?;

                SelectorFilter {
                    name: search_type,
                    attrs: vec![("name".into(), search_name)],
                    value: None,
                }
                .filter_children(context)
            }
            "findLike" => {
                let search_regex = get_attr!(node, bool, "regex", false)?;

                let search_type = get_attr!(node, StringFilter(search_regex), "type")?;

                SelectorFilter {
                    name: search_type,
                    ..SelectorFilter::from_selector_parent(node, search_regex)
                        .context("Failed to parse selector element")?
                }
                .filter_children(context)
            }
            "findWithChildLike" => {
                let search_regex = get_attr!(node, bool, "regex", false)?;

                let search_type = get_attr!(node, StringFilter(search_regex), "type")?;

                let search_child_type = get_attr!(node, StringFilter(search_regex), "child-type")?;

                WithChildFilter {
                    name: search_type,
                    child_filter: SelectorFilter {
                        name: search_child_type,
                        ..SelectorFilter::from_selector_parent(node, search_regex)
                            .context("Failed to parse selector element")?
                    },
                }
                .filter_children(context)
            }
            "findComposite" => {
                let Some(par) = node.get_child(("par", "mod")) else {
                    bail!("findComposite element is missing a par child");
                };

                let mut vec = mod_par(context, par)?.unwrap();
                // SAFETY: This is just an elaborate trick to get the address behind the shared reference.
                vec.sort_by_key(|x| unsafe { (x as *const &mut Element as *const *mut Element).read().addr() });
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
            let mut msg = format!("{} element has panic=true but no elements matched", node.name);

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
                    match &*command.name {
                        "selector" | "par" => {}
                        "setAttributes" => {
                            context
                                .attributes
                                .extend(command.attributes.iter().map(|(k, v)| (k.to_owned(), v.to_owned())));
                        }
                        "removeAttributes" => {
                            for key in command.attributes.keys() {
                                let _ = context.attributes.remove(key);
                            }
                        }
                        "setValue" => {
                            context
                                .children
                                .retain(|node| !matches!(node, XMLNode::CData(_) | XMLNode::Text(_)));
                            context.children.push(XMLNode::Text(command.get_text_trim()))
                        }
                        "removeTag" => {
                            context.prefix = Some(REMOVE_MARKER.into());
                        }
                        "insertByFind" => {
                            let elements = command.children.iter().filter_map(Node::as_element);

                            let add_anyway = get_attr!(command, bool, "addAnyway", true)?;

                            let mut found = None;
                            let mut before = Vec::new();
                            let mut after = Vec::new();
                            for child in elements.clone() {
                                // NOTE: This ignores all but the last mod:find tag but this is how slipstream does it.
                                if child.prefix.as_deref() == Some("mod") {
                                    if let Some(results) = mod_find(context, child)? {
                                        found = Some(results);
                                    } else {
                                        bail!(
                                            "insertByFind encountered unexpected <{}> tag (expected a <mod:find*> tag)",
                                            child.make_qualified_name()
                                        );
                                    }
                                } else if child.prefix.as_deref() == Some("mod-before") {
                                    let mut child = child.clone();
                                    child.prefix = None;
                                    before.push(child);
                                } else if child.prefix.as_deref() == Some("mod-after") {
                                    let mut child = child.clone();
                                    child.prefix = None;
                                    after.push(child);
                                } else {
                                    bail!(
                                        "insertByFind encountered unexpected tag <{}> (expected a <mod:find*>, <mod-before:*> or <mod-after:*> tag)",
                                        child.make_qualified_name()
                                    )
                                }
                            }

                            let Some(found) = found else {
                                bail!("insertByFind is missing a <mod:find*> tag");
                            };

                            if before.is_empty() && after.is_empty() {
                                bail!("insertByFind requires at least one <mod-before:*> or <mod-after:*> tag")
                            }

                            if found.is_empty() {
                                if add_anyway {
                                    context.children.splice(0..0, before.into_iter().map(Node::Element));
                                    context
                                        .children
                                        .splice(context.children.len().., after.into_iter().map(Node::Element));
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

                                let first = unwrap_ptr!(found.first().unwrap());
                                let last = unwrap_ptr!(found.last().unwrap());

                                debug_assert!(first.is_aligned() && last.is_aligned());
                                let range = context.children.as_mut_ptr_range();
                                debug_assert!(range.contains(&first));
                                debug_assert!(range.contains(&last));

                                // SAFETY: last and first should both point to the same allocation as `context.children`.
                                let first_idx = unsafe { first.offset_from(range.start) as usize };
                                let last_idx = unsafe { last.offset_from(range.start) as usize };

                                // FIXME: This insertion strategy is not optimal. (does it matter?)
                                let before_len = before.len();
                                context
                                    .children
                                    .splice(first_idx..first_idx, before.into_iter().map(Node::Element));
                                let after_insert_idx = last_idx + before_len + 1;
                                context
                                    .children
                                    .splice(after_insert_idx..after_insert_idx, after.into_iter().map(Node::Element));
                            }
                        }
                        _ => {
                            bail!("Unrecognised mod command tag {}", command.name)
                        }
                    }
                }
            }
            Some("mod-prepend") => {
                let mut new = command.clone();
                new.prefix = None;

                // FIXME: Linked list? Benchmarks needed.
                context.children.insert(0, XMLNode::Element(new));
            }
            Some("mod-append") => {
                let mut new = command.clone();
                new.prefix = None;

                context.children.push(XMLNode::Element(new));
            }
            Some("mod-overwrite") => {
                let mut new = command.clone();
                new.prefix = None;

                if let Some(old) = context.get_mut_child(&new.name) {
                    let _ = std::mem::replace(old, new);
                } else {
                    context.children.push(XMLNode::Element(new));
                }
            }
            Some(other) => bail!("Unrecognised mod command namespace: {:?}", other),
            None => bail!("Mod command is missing a namespace"),
        }
    }

    Ok(())
}
