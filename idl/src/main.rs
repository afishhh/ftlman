#![allow(dead_code)]

use std::{collections::HashSet, fmt::Display, io::Write};

#[derive(Debug, Clone, Copy, Hash, PartialEq, Eq, PartialOrd, Ord)]
struct Ident(&'static str);

impl Display for Ident {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.0)
    }
}

#[derive(Debug)]
struct Path(Vec<Ident>);

impl Path {
    fn new(ident: Ident) -> Self {
        Self(vec![ident])
    }

    fn join(&self, ident: Ident) -> Path {
        let mut new = self.0.clone();
        new.push(ident);
        Path(new)
    }
}

#[derive(Debug, Clone)]
struct GenericParams(Vec<Type>);

#[derive(Debug, Clone)]
enum Type {
    Any,
    Nil,
    Table(Table),
    Array(Array),
    String,
    Integer,
    Number,
    Function(Function),
    Literal(&'static str),
    Ident(IdentType),
    Union(Vec<Type>),
}

#[derive(Debug, Clone)]
enum MultiType {
    // number, string, Node
    Types(Vec<Type>),
    // ...T
    Spread(Ident),
    // (number, string) | (string, number)
    Union(Vec<MultiType>),
}

#[derive(Debug, Clone)]
struct Table {
    key: Box<Type>,
    value: Box<Type>,
}

#[derive(Debug, Clone)]
struct Array {
    value: Box<Type>,
}

#[derive(Debug, Clone)]
struct IdentType {
    name: Ident,
    params: GenericParams,
}

#[derive(Debug, Clone)]
struct Function {
    params: Vec<(Ident, Type)>,
    variadic: Option<MultiType>,
    output: Box<Type>,
}

#[derive(Debug)]
struct FunctionItem {
    name: Ident,
    doc_comment: Option<String>,
    generics: Vec<Ident>,
    function: Function,
}

#[derive(Debug)]
struct Field {
    readonly: bool,
    doc_comment: Option<String>,
    name: Ident,
    type_: Type,
}

#[derive(Debug)]
struct ClassItem {
    name: Ident,
    parent: Option<Ident>,
    fields: Vec<Field>,
    methods: Vec<FunctionItem>,
}

#[derive(Debug)]
struct AliasItem {
    name: Ident,
    inner: Type,
}

#[derive(Debug)]
struct Library {
    path: Path,
    name: &'static str,
    items: Vec<Item>,
}

#[derive(Debug)]
enum Item {
    Alias(AliasItem),
    Function(FunctionItem),
    Class(ClassItem),
}

#[derive(Debug)]
struct Types {
    libraries: Vec<Library>,
    items: Vec<Item>,
}

fn flatten_unions(a: Type, b: Type) -> Type {
    let mut types = match (a, b) {
        (Type::Union(mut ta), Type::Union(tb)) => {
            ta.extend(tb);
            ta
        }
        (Type::Union(mut ta), tb) => {
            ta.push(tb);
            ta
        }
        (ta, Type::Union(mut tb)) => {
            tb.push(ta);
            tb
        }
        (ta, tb) => vec![ta, tb],
    };

    let mut seen_nil = false;
    types.retain(|value| {
        if matches!(value, Type::Nil) {
            if seen_nil {
                return false;
            }
            seen_nil = true;
        }

        true
    });

    Type::Union(types)
}

fn merge_doc_comment(comment: &'static [&'static str]) -> String {
    let mut result = String::new();
    for line in comment {
        let trimmed = line.trim();
        result.push_str(trimmed);
        result.push('\n')
    }
    result
}

fn split_paragraphs(text: &str) -> impl Iterator<Item = String> {
    text.split("\n\n").map(|p| p.trim().replace("\n", " "))
}

// Glorious TT muncher
macro_rules! types {
    // Library declaration
    (@toplevel [$types: ident] library [$($path: tt)*] $name: literal { $($tt: tt)* } $($rest: tt)*) => {
        types!(@library [$types [$($path)*] $name] $($tt)*);
        types!(@toplevel [$types] $($rest)*);
    };
    (@toplevel [$current: ident] $($rest: tt)+) => {
        types!(@item toplevel [$current] $($rest)*);
    };

    // Class declaration
    (@item $next_state: ident [$current: ident] class $name: ident $(: $parent: ident)? { $($tt: tt)* } $($rest: tt)*) => {
        types!(@class [$current $($parent)?] $name $($tt)*);
        types!(@$next_state [$current] $($rest)*);
    };
    // Type alias declaration
    (@item $next_state: ident [$current: ident] type $name: ident = $($rest: tt)*) => {
        types!(@type1 [typealias_return $next_state $current $name] $($rest)*);
    };
    // Function declaration
    (@method_or_function
        [$($args: tt)*]
        $name: ident < $($rest: tt)*
    ) => {
        types!(@genericslist [method_or_function_params $($args)* $name] $($rest)*);
    };
    (@method_or_function
        [$($args: tt)*]
        $name: ident $($rest: tt)*
    ) => {
        types!(@method_or_function_params [$($args)* $name] {Vec::new()} > $($rest)*);
    };
    (@method_or_function_params
        [$($args: tt)*]
        $generics: tt
        > ($($params: tt)*) -> $($rest: tt)*
    ) => {
        types!(@type1 [method_or_function_final $($args)* $generics $($params)*] $($rest)*)
    };
    (@method_or_function_final
        [$next_state: ident $current: ident.$field: ident $({$($fun: tt)*})? [$($comment: literal)*] $method: ident $generics: tt $($params: tt)*]
        [$($return_type: tt)*];
        $($rest: tt)*
    ) => {
        $current.$field.push($($($fun)*)?(FunctionItem {
            name: Ident(stringify!($method)),
            generics: $generics,
            doc_comment: types!(@doc_comment_or_none $($comment)*),
            function: {
                let mut function = Function {
                    params: Vec::new(),
                    variadic: None,
                    output: Box::new($($return_type)*)
                };
                types!(@params [function] $($params)*);
                function
            }
        }));
        types!(@$next_state [$current] $($rest)*)
    };

    // Function params
    (@params [$current: ident] ...: $($rest: tt)*) => {
        // TODO: This should be a @multi_type instead
        types!(@type1 [variadic_set $current] $($rest)*)
    };
    (@params [$current: ident] $name: ident: $($rest: tt)*) => {
        types!(@type1 [param_return $current $name] $($rest)*)
    };
    (@params [$current: ident]) => {};
    (@variadic_set [$current: ident] [$($rest: tt)*]) => {
        assert!($current.variadic.is_none());
        $current.variadic = Some(MultiType::Types(vec![$($rest)*]));
    };
    (@param_return [$current: ident $name: ident] [$($type: tt)*], $($rest: tt)*) => {
        types!(@param_return [$current $name] [$($type)*]);
        types!(@params [$current] $($rest)*)
    };
    (@param_return [$current: ident $name: ident] [$($type: tt)*]) => {
        $current.params.push((Ident(stringify!($name)), $($type)*));
    };

    (@typealias_return [$next_state: ident $library: ident $name: ident] [$($type: tt)*]; $($rest: tt)*) => {
        $library.items.push(Item::Alias(AliasItem {
            name: Ident(stringify!($name)),
            inner: $($type)*
        }));
        types!(@$next_state [$library] $($rest)*)
    };


    // we're done! (finally)
    (@toplevel [$types: ident]) => { };

    (@basic_type any Vec::new()) => { Type::Any };
    (@basic_type nil Vec::new()) => { Type::Nil };
    (@basic_type string Vec::new()) => { Type::String };
    (@basic_type integer Vec::new()) => { Type::Integer };
    (@basic_type number Vec::new()) => { Type::Number };
    (@basic_type Array $($generics: tt)*) => {
        Type::Array({
            let mut generics = $($generics)*;
            assert_eq!(generics.len(), 1);
            Array {
                value: Box::new(generics[0].clone()),
            }
        })
    };
    (@basic_type Table $($generics: tt)*) => {
        Type::Table({
            let mut generics: Vec<Type> = $($generics)*;
            if generics.is_empty() {
                Table {
                    key: Box::new(Type::Any),
                    value: Box::new(Type::Any),
                }
            } else {
                assert_eq!(generics.len(), 2);
                Table {
                    key: Box::new(generics[0].clone()),
                    value: Box::new(generics[1].clone())
                }
            }
        })
    };
    (@basic_type $identifier: ident $($generics: tt)*) => {
        Type::Ident(IdentType {
            name: Ident(stringify!($identifier)),
            params: GenericParams($($generics)*)
        })
    };

    // First pass of type construction
    // Parametrized generic ident type
    (@type1 [$($stargs: tt)*] $identifier: ident < $($rest: tt)*) => {
        types!(@typelist [type1_return_from_generics $identifier $($stargs)*] $($rest)*);
    };
    (@type1_return_from_generics [$identifier: ident $($stargs: tt)*] $tt: tt > $($rest: tt)*) => {
        types!(@type2 [$($stargs)*] [types!(@basic_type $identifier $tt)] $($rest)*)
    };

    // Function type
    (@type1 [$($stargs: tt)*] Fn($($params: tt)*) -> $($rest: tt)*) => {
        types!(@type1 [type1_return_from_function [$($stargs)*] $($params)*] $($rest)*)
    };
    (@type1 [$($stargs: tt)*] Fn($($params: tt)*)) => {
        types!(@type1_return_from_function [[$($stargs)*] $($params)*] Type::Nil)
    };
    (@type1_return_from_function [[$next_state: ident $($stargs: tt)*] $($params: tt)*] [$($return_type: tt)*] $($rest: tt)*) => {
        types!(@$next_state [$($stargs)*] [
            Type::Function({
                let mut function = Function {
                    params: Vec::new(),
                    variadic: None,
                    output: Box::new($($return_type)*)
                };
                types!(@params [function] $($params)*);
                function
            })
        ] $($rest)*)
    };

    // Simple identifier type
    (@type1 [$($stargs: tt)*] $identifier: ident $($rest: tt)*) => {
        types!(@type2 [$($stargs)*] [types!(@basic_type $identifier Vec::new())] $($rest)*)
    };

    // Parenthesised type
    (@type1 [$($stargs: tt)*] ( $($type: tt)* ) $($rest: tt)*) => {
        types!(@type1 [type1_return_from_parens [$($rest)*] $($stargs)*] $($type)*)
    };
    (@type1_return_from_parens [[$($rest: tt)*] $($stargs: tt)*] [$($type: tt)*]) => {
        types!(@type2 [$($stargs)*] [$($type)*] $($rest)*)
    };
    (@type1 [$($stargs: tt)*] $literal: literal $($rest: tt)*) => {
        types!(@type2 [$($stargs)*] [Type::Literal(stringify!($literal))] $($rest)*)
    };

    // Second pass of type construction, handle optionals and unions
    // NOTE: Currently (string | number?) results in (string | number)? instead.
    //       Fixing this is not necessary because those reduce to the exact same
    //       union with nil.
    (@type2 [$($stargs: tt)*] [$($type: tt)*] ? $($rest: tt)*) => {
        types!(@type2 [$($stargs)*] [
            flatten_unions($($type)*, Type::Nil)
        ] $($rest)*);
    };
    // Type union
    (@type2 [$($stargs: tt)*] [$($type: tt)*] | $($rest: tt)*) => {
        types!(@type1 [type2_union_return [$($type)*] $($stargs)*] $($rest)*)
    };
    (@type2_union_return [[$($lhs: tt)*] $($stargs: tt)*] [$($rhs: tt)*] $($rest: tt)*) => {
        types!(@type2 [$($stargs)*] [flatten_unions($($lhs)*, $($rhs)*)] $($rest)*)
    };
    (@type2 [$next_state: ident $($stargs: tt)*] [$($type: tt)*] $($rest: tt)*) => {
        types!(@$next_state [$($stargs)*] [$($type)*] $($rest)*);
    };

    // FIXME: Maybe this can be cleaned up?
    (@typelist [$($stargs: tt)*] $($rest: tt)*) => {
        types!(@type1 [typelist_rec acc {} $($stargs)*] $($rest)*)
    };
    (@typelist_rec [$acc: ident { $($adds: tt)* } $($stargs: tt)*] [$($type: tt)*], $($rest: tt)*) => {
        types!(@type1 [typelist_rec $acc {
            $($adds)*
            $acc.push($($type)*);
        } $($stargs)*]  $($rest)*)
    };
    (@typelist_rec [$acc: ident { $($adds: tt)* } $next_state: ident $($stargs: tt)*] [$($type: tt)*] $($rest: tt)*) => {
        types!(@$next_state [$($stargs)*] {
            let mut $acc = Vec::new();
            $($adds)*
            $acc.push($($type)*);
            $acc
        } $($rest)*)
    };

    // FIXME: Maybe this can be merged into typelist?
    //        Some comma_separated_list macro method?
    (@genericslist [$($stargs: tt)*] $($rest: tt)*) => {
        types!(@genericslist_rec [acc {} $($stargs)*] $($rest)*)
    };
    (@genericslist_rec [$acc: ident { $($adds: tt)* } $($stargs: tt)*] $ident: ident, $($rest: tt)*) => {
        types!(@genericslist_rec [$acc {
            $($adds)*
            $acc.push(Ident(stringify!($ident)));
        } $($stargs)*] $($rest)*)
    };
    (@genericslist_rec [$acc: ident { $($adds: tt)* } $next_state: ident $($stargs: tt)*] $ident: ident $($rest: tt)*) => {
        types!(@$next_state [$($stargs)*] {
            let mut $acc = Vec::new();
            $($adds)*
            $acc.push(Ident(stringify!($ident)));
            $acc
        } $($rest)*)
    };


    (@class [$types: ident $($parent: ident)?] $name: ident $($tt: tt)*) => {
        let mut current_class = ClassItem {
            name: Ident(stringify!($name)),
            parent: types!(@classparent $($parent)?),
            fields: Vec::new(),
            methods: Vec::new()
        };
        types!(@classinner [current_class] $($tt)*);
        $types.items.push(Item::Class(current_class));
    };
    (@classparent) => { None };
    (@classparent $ident: ident) => { Some(Ident(stringify!($ident))) };

    (@classinner [$current: ident]) => { };
    // HACK: this allows to "switch" treesitter to "function mode" after fields
    (@classinner [$current: ident]; $($rest: tt)*) => {
        types!(@classinner [$current] $($rest)*)
    };

    // Fields
    (@classinner [$current: ident]
        $(#[doc = $comment: literal])*
        #[readonly] $field: ident: $($rest: tt)*
    ) => {
        types!(@type1 [classfield $current $field true $($comment)*] $($rest)*);
    };
    (@classinner [$current: ident]
        $(#[doc = $comment: literal])*
        $field: ident: $($rest: tt)*
    ) => {
        types!(@type1 [classfield $current $field false $($comment)*] $($rest)*);
    };
    (@classfield [$current: ident $($args: tt)*] [$($type: tt)*], $($rest: tt)*) => {
        types!(@classfield [$current $($args)*] [$($type)*]);
        types!(@classinner [$current] $($rest)*)
    };
    (@classfield [$current: ident $field: ident $readonly: literal $($description: literal)*] [$($type: tt)*]) => {
        $current.fields.push(Field {
            readonly: $readonly,
            doc_comment: types!(@doc_comment_or_none $($description)?),
            name: Ident(stringify!($field)),
            type_: $($type)*
        });
    };

    (@doc_comment_or_none $($literals: literal)+) => { Some(merge_doc_comment(&[$($literals),+])) };
    (@doc_comment_or_none) => { None };

    // Methods
    (@classinner [$current: ident]
        $(#[doc = $comment: literal])*
        fn $($rest: tt)*
    ) => {
        types!(@method_or_function [classinner $current.methods [$($comment)*]] $($rest)*);
    };

    (@library [$types: ident [$first: ident $($path: tt)*] $name: literal] $($tt: tt)*) => {
        let mut library = Library {
            name: $name,
            path: types!(@path [Path::new(Ident(stringify!($first)))] $($path)*),
            items: Vec::new(),
        };

        types!(@libraryinner [library] $($tt)*);

        $types.libraries.push(library);
    };
    // Function declaration
    (@libraryinner [$current: ident]
        $(#[doc = $comment: literal])*
        fn $($rest: tt)*
    ) => {
        types!(@method_or_function [libraryinner $current.items {Item::Function} [$($comment)*]] $($rest)*)
    };
    (@libraryinner [$current: ident] $($rest: tt)+) => {
        types!(@item libraryinner [$current] $($rest)*)
    };
    (@libraryinner [$current: ident]) => { };

    (@path [$path: expr] . $component: ident $($rest: tt)*) => {
        types!(@path [$path.join(Ident(stringify!($component)))] $($rest)*)
    };
    (@path [$path: expr]) => {
        $path
    };

    (@$state: ident $($tt: tt)*) => {
        compile_error!(concat!(stringify!($state), " state failed to match with params: ", stringify!($($tt)*)))
    };
    ($name: ident; $($tt: tt)*) => {
        #[allow(unused_mut)]
        #[allow(clippy::vec_init_then_push)]
        fn $name() -> Types {
            let mut types = Types {
                libraries: Vec::new(),
                items: Vec::new()
            };

            // start munchin'
            types!(@toplevel [types] $($tt)*);

            types
        }
    }
}

mod types_test {
    use super::*;

    types! {
        test_types;

        class Test {
            string_literal: "element",
            number_literal: 20,
            union1: string | number,
            parens: (string),
            union_parens: (string | 10 | "hello")?,
        }
    }
}

types! {
    document_section_types;

    class Document {
        root: Element
    }
}

types! {
    main_types;

    library [mod.xml] "DOM" {
        type NodeType = "element" | "text";

        class Node {
            #[readonly] type: NodeType,
            /// Previous sibling node of this node.
            #[readonly] previousSibling: Node?,
            /// Next sibling node of this node.
            #[readonly] nextSibling: Node?,
            /// Parent node of this node.
            #[readonly] parent: Element?,
            ;
            /// Attempts to cast this `Node` as an `Element`.
            /// Returns `nil` if this is node is not an `Element`.
            fn as(type: "element") -> Element?;
            /// Attempts to cast this `Node` as a `Text` node.
            /// Returns `nil` if this is node is not a `Text` node.
            fn as(type: "text") -> Text?;
            // TODO:
            // fn clone(mode: "deep" | "shallow") -> Node;
        }

        class Element: Node {
            #[readonly] type: "element",
            /// Name component of this node's XML tag.
            name: string,
            /// Prefix component of this node's XML tag.
            prefix: string,
            /// First child of this element that is also an element.
            #[readonly] firstElementChild: Element?,
            /// Last child of this element that is also an element.
            #[readonly] lastElementChild: Element?,
            /// First child node of this element.
            #[readonly] firstChild: Node?,
            /// Last child node of this element.
            #[readonly] lastChild: Node?,
            // FIXME: Currently this is a lie and only concatenates the direct children.
            /// Contents of all the text nodes in this element's subtree
            /// concatenated together.
            textContent: string,
            ;
            /// Returns an iterator over all immediate Element children of this Element.
            fn children() -> Fn() -> Element?;
            /// Returns an iterator over all immediate children of this Element.
            fn childNodes() -> Fn() -> Node?;
            /// Appends the specified nodes to the end of this Element's child list.
            /// String values are implicitly converted into text nodes.
            fn append(...: Node | string) -> nil;
            /// Prepends the specified nodes to the end of this Element's child list.
            /// String values are implicitly converted into text nodes.
            fn prepend(...: Node | string) -> nil;
            // TODO:
            // would execute an append script
            // maybe mode could also be "lua" or "rawxml"
            // fn execute(append_mode: "xml", script: string) -> nil;
            // TODO:
            // fn clone(mode: "deep" | "shallow") -> Element;
        }

        class Text: Node {
            #[readonly] type: "text",
            /// Text content of this text node.
            content: string,
        }

        /// Create a new `Element` with the specified prefixed name and attributes.
        fn element(prefix: string, name: string, attrs: Table<string, string>?)
            -> Element;
        /// Create a new `Element` with the specified name and attributes.
        fn element(name: string, attrs: Table<string, string>?)
            -> Element;
    }

    library [mod.util] "Utility" {
        // TODO: T: table (generic constraint)
        /// Returns `table` with its metatable replaced by one that disallows
        /// changing it.
        fn readonly<T>(table: T) -> T;
    }

    library [mod.iter] "Iterator" {
        /// Counts the number of elements returned by `iterator`.
        fn count<T>(iterator: Fn() -> T?) -> integer;
        /// Returns a new iterator that, for each value returned by `iterator`,
        /// returns the result of passing it to `mapper`.
        /// Iteration stops when either `iterator` or `mapper` first return `nil`.
        fn map<T, U>(iterator: Fn() -> T?, mapper: Fn(v: T) -> U?) -> U?;
        /// Returns all values returned by `iterator` as an array.
        fn collect<T>(iterator: Fn() -> T?) -> Array<T>;
        // TODO: MultiType support in return types
        // fn enumerate<T>(iterator: Fn() -> T?, start: number?) -> Fn() -> (integer, ...T)?;
        // fn zip<T, U>(a: Fn() -> T?, b: Fn() -> U?) -> Fn() -> (...T, ...U)?;
    }

    library [mod.table] "Table" {
        /// Returns a new iterator over the array part of table `array`.
        fn iter_array<T>(array: Array<T>) -> Fn() -> T?;
        /// Lexicographically compares the elements of arrays `a` and `b`.
        ///
        /// If the arrays are equal returns `0`.
        /// If array `a` is lexicographically smaller than `b` returns a negative
        /// number that is the negation of the position where the first mismatch occured.
        /// If array `a` is lexicographically greater than `b` returns the
        /// position where the mismatch occurred.
        fn compare_arrays<T>(a: Array<T>, b: Array<T>) -> integer;
    }

    library [mod.debug] "Debugging" {
        // TODO: strongly typed tables (interface type?)
        // type PrettyOptions = {
        //     recursive: bool,
        //     colors: "ansi" | nil,
        //     indent: string | nil
        // }
        /// Formats `value` in an unspecified human readable format according
        /// to the options in `options`.
        ///
        /// Returns the result as a string.
        /// The exact contents of this string must not be relied upon and may change.
        fn pretty_string(value: any, options: Table?) -> string;
        /// Prints `value` in an unspecified human readable format according
        /// to the options in `options`.
        fn pretty_print(value: any, options: Table?) -> nil;
        /// Raises an error if `a` is not equal to `b` according to an
        /// unspecified deep equality relation.
        fn assert_equal(a: any, b: any) -> nil;
    }
}

type Result<T, E = Box<dyn std::error::Error + Send + Sync>> = std::result::Result<T, E>;

fn type_to_luacats(out: &mut impl Write, type_: &Type) -> Result<()> {
    match type_ {
        Type::Any => write!(out, "any")?,
        Type::Nil => write!(out, "nil")?,
        Type::Array(Array { value }) => {
            type_to_luacats(out, value)?;
            write!(out, "[]")?;
        }
        Type::Table(Table { key, value }) => {
            write!(out, "table<")?;
            type_to_luacats(out, key)?;
            write!(out, ", ")?;
            type_to_luacats(out, value)?;
            write!(out, ">")?;
        }
        Type::String => write!(out, "string")?,
        Type::Integer => write!(out, "integer")?,
        Type::Number => write!(out, "number")?,
        Type::Literal(literal) => write!(out, "{literal}")?,
        Type::Ident(IdentType { name, params }) => {
            assert!(params.0.is_empty());
            write!(out, "{name}")?
        }
        Type::Function(function) => {
            write!(out, "fun(")?;
            let mut first = true;
            for (name, type_) in &function.params {
                if !first {
                    write!(out, ", ")?;
                } else {
                    first = false;
                }

                write!(out, "{name}: ")?;
                type_to_luacats(out, type_)?;
            }
            write!(out, "): ")?;
            type_to_luacats(out, &function.output)?;
        }
        Type::Union(vec) => {
            if let [tp, Type::Nil] = vec.as_slice() {
                type_to_luacats(out, tp)?;
                write!(out, "?")?;
            } else {
                write!(out, "(")?;
                let mut first = true;
                for type_ in vec {
                    if !first {
                        write!(out, " | ")?;
                    } else {
                        first = false;
                    }

                    type_to_luacats(out, type_)?;
                }
                write!(out, ")")?;
            }
        }
    }

    Ok(())
}

fn function_to_luacats(
    out: &mut impl Write,
    prefix: &str,
    FunctionItem {
        name,
        doc_comment,
        generics,
        function,
    }: &FunctionItem,
) -> Result<()> {
    if !generics.is_empty() {
        writeln!(
            out,
            "---@generic {}",
            generics.iter().map(Ident::to_string).collect::<Vec<_>>().join(", ")
        )?;
    }

    for (name, type_) in &function.params {
        write!(out, "---@param {name} ")?;
        type_to_luacats(out, type_)?;
        writeln!(out)?;
    }

    if let Some(variadic) = &function.variadic {
        write!(out, "---@params ... ")?;
        match variadic {
            MultiType::Types(vec) => {
                assert_eq!(vec.len(), 1);
                type_to_luacats(out, &vec[0])?;
            }
            _ => unimplemented!(),
        }
        writeln!(out)?;
    }

    write!(out, "---@return ")?;
    type_to_luacats(out, &function.output)?;
    writeln!(out)?;

    if let Some(doc) = doc_comment {
        for line in doc.lines() {
            writeln!(out, "--- {line}")?;
        }
    }

    write!(out, "function {prefix}{name}(")?;
    let mut first = true;
    for (name, _) in &function.params {
        if !first {
            write!(out, ", ")?;
        } else {
            first = false;
        }

        write!(out, "{name}")?;
    }

    if function.variadic.is_some() {
        if !first {
            write!(out, ", ")?;
        }
        write!(out, "...")?;
    }

    writeln!(out, ") end")?;

    Ok(())
}

fn item_to_luacats(out: &mut impl Write, prefix: &str, item: &Item) -> Result<()> {
    match item {
        Item::Alias(alias) => {
            let types = match &alias.inner {
                Type::Union(vec) => vec.as_slice(),
                _ => unimplemented!(),
            };
            writeln!(out, "---@alias {}", alias.name)?;
            for variant in types {
                let value = match variant {
                    Type::Literal(literal) => literal,
                    _ => unimplemented!(),
                };
                writeln!(out, "---| {value}")?;
            }
        }
        Item::Class(class) => {
            write!(out, "---@class (exact) {}", class.name)?;
            if let Some(parent) = class.parent {
                write!(out, ": {parent}")?;
            }
            writeln!(out)?;

            for &Field {
                readonly,
                ref doc_comment,
                name,
                ref type_,
            } in &class.fields
            {
                write!(out, "---@field {name} ")?;
                type_to_luacats(out, type_)?;
                if let Some(d) = doc_comment {
                    write!(out, " {}", d.trim_end().replace("\n", " "))?;
                }
                if readonly {
                    write!(out, " This field is readonly.")?;
                }
                writeln!(out)?;
            }

            writeln!(out, "local {} = {{}}", class.name)?;

            for function in &class.methods {
                writeln!(out)?;
                if let Some(doc) = &function.doc_comment {
                    for line in doc.lines() {
                        writeln!(out, "--- {line}")?;
                    }
                }
                function_to_luacats(out, &format!("{prefix}{}:", class.name), function)?;
            }
        }
        Item::Function(function) => {
            writeln!(out)?;
            function_to_luacats(out, prefix, function)?;
        }
    }

    Ok(())
}

fn types_to_luacats(out: &mut impl Write, types: &Types) -> Result<()> {
    writeln!(out, "---@meta")?;

    for item in &types.items {
        writeln!(out)?;
        item_to_luacats(out, "", item)?;
    }

    for library in &types.libraries {
        writeln!(out)?;

        let mut current = library.path.0[0].0.to_owned();
        writeln!(out, "---@diagnostic disable-next-line: lowercase-global")?;
        writeln!(out, "{current} = {{}}")?;
        for ident in &library.path.0[1..] {
            writeln!(out, "{current}.{ident} = {{}}")?;
            current.push('.');
            current.push_str(ident.0);
        }

        for item in &library.items {
            writeln!(out)?;
            item_to_luacats(out, &format!("{current}."), item)?;
        }
    }

    Ok(())
}

const KW_TYPE: &str = r#"<span style="color: orangered">type</span>"#;
const KW_CLASS: &str = r#"<span style="color: orangered">class</span>"#;
const KW_FN: &str = r#"<span style="color: orangered">fn</span>"#;

const LIT_STR_START: &str = r#"<span style="color: limegreen">"#;
const LIT_NUM_START: &str = r#"<span style="color: limegreen">"#;
const LIT_END: &str = r#"</span>"#;

const ATTR_READONLY: &str = r#"<span style="color: lightseagreen">readonly</span>"#;

const COMMENT_START: &str = r#"<span class="comment" style="color: gray">"#;
const COMMENT_END: &str = r#"</span>"#;

fn span_class_type(s: &str) -> String {
    format!("<span style=\"color: gold;\">{s}</span>")
}

fn span_and_anchor_class_type(s: &str) -> String {
    format!("<span id=\"class-{s}\">{}</span>", span_class_type(s))
}

fn span_and_link_class_type(s: &str) -> String {
    format!(
        "<a href=\"#class-{s}\" style=\"text-decoration: underline 1px orange;\">{}</a>",
        span_class_type(s)
    )
}

fn span_primitive_type(s: &str) -> String {
    format!("<span style=\"color: dodgerblue;\">{s}</span>")
}

fn span_param_name(s: &str) -> String {
    format!("<span style=\"color: skyblue\">{s}</span>")
}

fn type_to_kramdown(out: &mut impl Write, type_: &Type, do_not_link: &HashSet<String>) -> Result<()> {
    match type_ {
        Type::Any => write!(out, "{}", span_primitive_type("any"))?,
        Type::Nil => write!(out, "{}", span_primitive_type("nil"))?,
        Type::Array(Array { value }) => {
            type_to_kramdown(out, value, do_not_link)?;
            write!(out, "[]")?;
        }
        Type::Table(Table { key, value }) => {
            write!(out, "{}", span_class_type("Table"))?;
            if !matches!(**key, Type::Any) || !matches!(**value, Type::Any) {
                write!(out, "<")?;
                type_to_kramdown(out, key, do_not_link)?;
                write!(out, ", ")?;
                type_to_kramdown(out, value, do_not_link)?;
                write!(out, ">")?;
            }
        }
        Type::String => write!(out, "{}", span_primitive_type("string"))?,
        Type::Integer => write!(out, "{}", span_primitive_type("integer"))?,
        Type::Number => write!(out, "{}", span_primitive_type("number"))?,
        Type::Literal(literal) => {
            if literal.starts_with('"') {
                write!(out, "{LIT_STR_START}")?;
            } else {
                write!(out, "{LIT_NUM_START}")?;
            }
            write!(out, "{literal}")?;
            write!(out, "{LIT_END}")?;
        }
        Type::Ident(IdentType { name, params }) => {
            assert!(params.0.is_empty());
            if do_not_link.contains(name.0) {
                write!(out, "{}", span_class_type(name.0))?
            } else {
                write!(out, "{}", span_and_link_class_type(name.0))?
            }
        }
        Type::Function(function) => {
            write!(out, "{KW_FN}(")?;
            let mut first = true;
            for (name, type_) in &function.params {
                if !first {
                    write!(out, ", ")?;
                } else {
                    first = false;
                }

                write!(out, "{}: ", span_param_name(name.0))?;
                type_to_kramdown(out, type_, do_not_link)?;
            }
            write!(out, "): ")?;
            type_to_kramdown(out, &function.output, do_not_link)?;
        }
        Type::Union(vec) => {
            if let [tp, Type::Nil] = vec.as_slice() {
                type_to_kramdown(out, tp, do_not_link)?;
                write!(out, "?")?;
            } else {
                write!(out, "(")?;
                let mut first = true;
                for type_ in vec {
                    if !first {
                        write!(out, " | ")?;
                    } else {
                        first = false;
                    }

                    type_to_kramdown(out, type_, do_not_link)?;
                }
                write!(out, ")")?;
            }
        }
    }

    Ok(())
}

fn write_ntabs(out: &mut impl Write, count: u64) -> Result<()> {
    for _ in 0..count {
        write!(out, "\t")?
    }

    Ok(())
}

fn doc_comment_to_kramdown(out: &mut impl Write, indent: u64, comment: &str) -> Result<()> {
    write!(out, "{COMMENT_START}")?;
    let first_indent = comment
        .lines()
        .find(|x| !x.trim().is_empty())
        .map(|x| x.len() - x.trim_start().len())
        .unwrap_or(0);
    for line in split_paragraphs(comment) {
        write_ntabs(out, indent)?;
        writeln!(out, "/// {}", &line[first_indent.min(line.len())..])?;
    }
    write!(out, "{COMMENT_END}")?;

    Ok(())
}

fn function_to_kramdown(
    out: &mut impl Write,
    indent: u64,
    prefix: &str,
    FunctionItem {
        name,
        doc_comment,
        generics,
        function,
    }: &FunctionItem,
) -> Result<()> {
    if let Some(comment) = doc_comment {
        doc_comment_to_kramdown(out, indent, comment)?
    }

    write_ntabs(out, indent)?;
    write!(out, "{KW_FN} {prefix}{name}")?;

    if !generics.is_empty() {
        write!(
            out,
            "&lt;{}&gt;",
            generics
                .iter()
                .map(|g| span_class_type(g.0))
                .collect::<Vec<_>>()
                .join(", ")
        )?;
    }
    let generic_set = generics.iter().map(|x| x.0.to_owned()).collect::<HashSet<String>>();

    write!(out, "(")?;
    let mut first = true;
    for (name, type_) in &function.params {
        if !first {
            write!(out, ", ")?;
        } else {
            first = false;
        }

        write!(out, "{}: ", span_param_name(name.0))?;
        type_to_kramdown(out, type_, &generic_set)?;
    }

    if let Some(type_) = &function.variadic {
        if !first {
            write!(out, ", ")?;
        }
        write!(out, "...: ")?;
        let type_ = match type_ {
            MultiType::Types(vec) => &vec[0],
            _ => todo!(),
        };
        type_to_kramdown(out, type_, &generic_set)?;
    }

    write!(out, ") -> ")?;
    type_to_kramdown(out, &function.output, &generic_set)?;
    writeln!(out, ";")?;

    Ok(())
}

fn item_to_kramdown(out: &mut impl Write, indent: u64, prefix: &str, item: &Item) -> Result<()> {
    match item {
        Item::Alias(alias) => {
            write!(out, "{KW_TYPE} {} = ", span_and_anchor_class_type(alias.name.0))?;
            type_to_kramdown(out, &alias.inner, &HashSet::new())?;
            writeln!(out, ";")?;
        }
        Item::Function(function) => {
            function_to_kramdown(out, indent, prefix, function)?;
        }
        Item::Class(class) => {
            write_ntabs(out, indent)?;
            write!(out, "{KW_CLASS} {}", span_and_anchor_class_type(class.name.0))?;
            if let Some(parent) = &class.parent {
                write!(out, ": {}", span_and_link_class_type(parent.0))?;
            }
            writeln!(out, " {{")?;

            for &Field {
                readonly,
                ref doc_comment,
                name,
                ref type_,
            } in &class.fields
            {
                if let Some(comment) = doc_comment {
                    doc_comment_to_kramdown(out, indent + 1, comment)?
                }

                write_ntabs(out, indent + 1)?;
                if readonly {
                    write!(out, "{} ", ATTR_READONLY)?;
                }
                write!(out, ".{name}: ")?;
                type_to_kramdown(out, type_, &HashSet::new())?;
                writeln!(out, ";")?;
            }

            if !class.methods.is_empty() {
                writeln!(out)?;
            }

            for item in &class.methods {
                function_to_kramdown(out, indent + 1, ":", item)?;
            }

            write_ntabs(out, indent)?;
            writeln!(out, "}}")?;
        }
    }

    Ok(())
}

fn items_to_kramdown(out: &mut impl Write, indent: u64, prefix: &str, items: &[Item]) -> Result<()> {
    if items.is_empty() {
        return Ok(());
    }

    write!(out, "<pre>")?;
    let mut first = true;
    for item in items {
        if !first {
            writeln!(out)?;
        } else {
            first = false;
        }

        item_to_kramdown(out, indent, prefix, item)?;
    }
    write!(out, "</pre>")?;

    Ok(())
}

fn types_to_kramdown(out: &mut impl Write, types: &Types) -> Result<()> {
    items_to_kramdown(out, 0, "", &types.items)?;

    for library in &types.libraries {
        writeln!(out)?;
        writeln!(out)?;
        writeln!(out, "## {} library", library.name)?;
        writeln!(out)?;
        writeln!(out)?;

        let prefix = library.path.0.iter().fold(String::new(), |mut s, i| {
            s.push_str(i.0);
            s.push('.');
            s
        });
        items_to_kramdown(out, 0, &prefix, &library.items)?;
    }

    Ok(())
}

fn write_kramdown_docs(out: &mut impl Write) -> Result<()> {
    write!(
        out,
        r#"# ftlman scriptable Lua patching API

Since ftlman v0.5 (unreleased) supports running Lua scripts during patching and provides
a Lua API that allows you to programatically change files in the FTL
data archive.

## Entrypoints and semantics

Two entrypoints are available to run Lua scripts during patching:

### Lua scripts

WIP: standalone exeuction not tied to a root element, semantics not yet concrete.

### Lua append scripts

Lua append scripts are lua equivalents of `.append.xml` files.
They have to be named `<some existing xml file without .xml>.append.lua` and will allow you to
modify the existing xml file whose file stem you substituted.

These scripts will have an additional global defined at runtime called `document`.
This global will be of type {}, described below.
Most important of all, `document` lets you access and modify the DOM tree of the XML file
by accessing the `.root` property.
All modifications done to the DOM will be written to the existing file after
the script finishes running.
"#,
        span_and_link_class_type("Document")
    )?;

    types_to_kramdown(out, &document_section_types())?;

    write!(
        out,
        r#"

#### Technical remarks
- This file will be executed alongside `.append.xml` files, and the order in which
these scripts run is currently unspecified.
- Note that as with normal `.append.xml` files, these won't be executed if the backing
XML file doesn't exist.

## Script environment

Apart from the potential `document` global, the `mod` global will always be present
in the script's environment.
It serves as the bridge between the mod manager and your scripts. The API
currently exposed by this global is described under the various "library" sections below.

        "#
    )?;

    types_to_kramdown(out, &main_types())?;

    Ok(())
}

fn main() {
    // types_to_luacats(&mut std::io::stdout(), &types).unwrap();
    write_kramdown_docs(&mut std::io::stdout()).unwrap();
}
