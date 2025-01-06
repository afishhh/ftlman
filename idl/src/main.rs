#![allow(dead_code)]

use std::{fmt::Display, io::Write};

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
    Union(Ident),
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
    functions: Vec<FunctionItem>,
}

#[derive(Debug)]
enum Item {
    Alias(AliasItem),
    Library(Library),
    Class(ClassItem),
}

#[derive(Debug)]
struct Types {
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

// Glorious TT muncher
macro_rules! types {
    // Class declaration
    (@toplevel [$types: ident] class $name: ident $(: $parent: ident)? { $($tt: tt)* } $($rest: tt)*) => {
        types!(@class [$types $($parent)?] $name $($tt)*);
        types!(@toplevel [$types] $($rest)*);
    };
    // Type alias declaration
    (@toplevel [$types: ident] type $name: ident = $($rest: tt)*) => {
        types!(@type1 [typealias_return $types $name] $($rest)*);
    };
    // Library declaration
    (@toplevel [$types: ident] library [$($path: tt)*] $name: literal { $($tt: tt)* } $($rest: tt)*) => {
        types!(@library [$types [$($path)*] $name] $($tt)*);
        types!(@toplevel [$types] $($rest)*);
    };

    // we're done! (finally)
    (@toplevel [$types: ident]) => { };

    (@typealias_return [$types: ident $name: ident] [$($type: tt)*]; $($rest: tt)*) => {
        $types.items.push(Item::Alias(AliasItem {
            name: Ident(stringify!($name)),
            inner: $($type)*
        }));
        types!(@toplevel [$types] $($rest)*)
    };

    (@basic_type any) => { Type::Any };
    (@basic_type nil) => { Type::Nil };
    (@basic_type string) => { Type::String };
    (@basic_type integer) => { Type::Integer };
    (@basic_type number) => { Type::Number };
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
            let mut generics = $($generics)*;
            assert_eq!(generics.len(), 2);
            Table {
                key: Box::new(generics[0].clone()),
                value: Box::new(generics[1].clone())
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
        [$next_state: ident $current: ident.$field: ident  [$($comment: literal)*] $method: ident $generics: tt $($params: tt)*]
        [$($return_type: tt)*];
        $($rest: tt)*
    ) => {
        $current.$field.push(FunctionItem {
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
        });
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

    (@library [$types: ident [$first: ident $($path: tt)*] $name: literal] $($tt: tt)*) => {
        let mut library = Library {
            name: $name,
            path: types!(@path [Path::new(Ident(stringify!($first)))] $($path)*),
            functions: Vec::new(),
        };

        types!(@libraryinner [library] $($tt)*);

        $types.items.push(Item::Library(library));
    };
    (@libraryinner [$current: ident]
        $(#[doc = $comment: literal])*
        fn $($rest: tt)*
    ) => {
        types!(@method_or_function [libraryinner $current.functions [$($comment)*]] $($rest)*)
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
    ($($tt: tt)*) => {
        #[allow(unused_mut)]
        #[allow(clippy::vec_init_then_push)]
        fn create_types() -> Types {
            let mut types = Types {
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
        fn as(type: "element") -> Element?;
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
        /// Content of all the text nodes in this element's subtree
        /// concatenated together.
        textContent: string,
        ;
        fn children() -> Fn() -> Element?;
        fn childNodes() -> Fn() -> Node?;
        fn append(...: Node | string) -> nil;
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

    library [mod.xml] "DOM" {
        /// Create a new `Element` the specified prefixed name and attributes.
        fn element(prefix: string, name: string, attrs: Table<string, string>?)
            -> Element;
        /// Create a new `Element` the specified name and attributes.
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
        fn map<T, U>(iterator: Fn() -> T?, mapper: Fn(v: T) -> U?) -> U?;
        /// Returns all values returned by `iterator` as an array.
        fn collect<T>(iterator: Fn() -> T?) -> Array<T>;
        // TODO: MultiType support in return types
        // fn enumerate<T>(iterator: Fn() -> T?, start: number?) -> Fn() -> (integer, ...T)?;
        // fn zip<T, U>(a: Fn() -> T?, b: Fn() -> T?) -> Fn() -> (integer, ...T);
    }

    library [mod.table] "Table" {
        /// Returns a new iterator over the array part of table `array`.
        fn iter_array<T>(array: Array<T>) -> Fn() -> T?;
        /// Lexicographically compares elements of arrays `a` and `b`.
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
        /// Returns the result as a string.
        /// The exact contents of this string must not be relied upon and may change.
        fn pretty_string(value: any, options: table?) -> string;
        /// Prints `value` in an unspecified human readable format according
        /// to the options in `options`.
        fn pretty_print(value: any, options: table?) -> nil;
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

fn function_to_luacats(out: &mut impl Write, name: &str, generics: &[Ident], fun: &Function) -> Result<()> {
    if !generics.is_empty() {
        writeln!(
            out,
            "---@generic {}",
            generics.iter().map(Ident::to_string).collect::<Vec<_>>().join(", ")
        )?;
    }

    for (name, type_) in &fun.params {
        write!(out, "---@param {name} ")?;
        type_to_luacats(out, type_)?;
        writeln!(out)?;
    }

    if let Some(variadic) = &fun.variadic {
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
    type_to_luacats(out, &fun.output)?;
    writeln!(out)?;

    write!(out, "function {name}(")?;
    let mut first = true;
    for (name, _) in &fun.params {
        if !first {
            write!(out, ", ")?;
        } else {
            first = false;
        }

        write!(out, "{name}")?;
    }

    if fun.variadic.is_some() {
        if !first {
            write!(out, ", ")?;
        }
        write!(out, "...")?;
    }

    writeln!(out, ") end")?;

    Ok(())
}

fn item_to_luacats(out: &mut impl Write, item: &Item) -> Result<()> {
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

            for FunctionItem {
                name,
                doc_comment,
                generics,
                function: fun,
            } in &class.methods
            {
                writeln!(out)?;
                if let Some(doc) = doc_comment {
                    for line in doc.lines() {
                        writeln!(out, "--- {line}")?;
                    }
                }
                function_to_luacats(out, &format!("{}:{name}", class.name), generics, fun)?;
            }
        }
        Item::Library(library) => {
            let mut current = library.path.0[0].0.to_owned();
            writeln!(out, "---@diagnostic disable-next-line: lowercase-global")?;
            writeln!(out, "{current} = {{}}")?;
            for ident in &library.path.0[1..] {
                writeln!(out, "{current}.{ident} = {{}}")?;
                current.push('.');
                current.push_str(ident.0);
            }

            for FunctionItem {
                name,
                doc_comment,
                generics,
                function,
            } in &library.functions
            {
                writeln!(out)?;
                if let Some(doc) = doc_comment {
                    for line in doc.lines() {
                        writeln!(out, "--- {line}")?;
                    }
                }
                function_to_luacats(out, &format!("{current}.{name}"), generics, function)?;
            }
        }
    }

    Ok(())
}

fn types_to_luacats(out: &mut impl Write, types: &Types) -> Result<()> {
    writeln!(out, "---@meta")?;

    for item in &types.items {
        writeln!(out)?;
        item_to_luacats(out, item)?;
    }

    Ok(())
}

fn main() {
    let types = dbg!(create_types());
    types_to_luacats(&mut std::io::stdout(), &types).unwrap();
}
