---@meta

---@class _ModApi
mod = {
  xml = {},
  util = {},
  iter = {},
  table = {},
  debug = {}
}

---@generic T: table
---@param table T
---@return T
function mod.util.readonly(table) end

---@generic T
---@param iterator fun(): T?
---@param start? number
---@return fun(): [integer, T]?
function mod.iter.enumerate(iterator, start) end

---@generic T, U
---@param mapper fun(v: T): U?
---@param iterator fun(): T?
---@return fun(): U?
function mod.iter.map(iterator, mapper)
  return mapper(iterator())
end

---@generic T
---@param iterator fun(): T?
---@return T[]
function mod.iter.collect(iterator) end

---@generic T
---@param array T[]
---@return fun(): T?
function mod.table.iter_array(array) end

---@generic T
---@param a T[]
---@param b T[]
---@return integer
function mod.table.compare_arrays(a, b) end

---@generic T
---@param a T
---@param b T
---@return nil
function mod.debug.assert_equal(a, b) end

---@class Document
document = {}

---@type Element
---@diagnostic disable-next-line: missing-fields
document.root = {}

---@alias NodeType
---| 'element' An XML element
---| 'text' An XML text node

---@class (exact) Node
---@field type NodeType
---@field previousSibling Node?
---@field nextSibling Node?
---@field parent Element?
local Node = {}

---@param type 'element'
---@return Element?
function Node:as(type) end

---@param type 'text'
---@return Text?
function Node:as(type) end

---@class (exact) Element: Node
---@field type 'element'
---@field name string
---@field prefix string
---@field firstElementChild Element?
---@field lastElementChild Element?
---@field firstChild Node?
---@field lastChild Node?
---@field textContent string
local Element = {}

---@return fun(): Element
function Element:children() end

---@return fun(): Node
function Element:childNodes() end

---@param ... Node|string Nodes to append as children of this element
function Element:append(...) end

---@param ... Node|string Nodes to prepend as children of this element
function Element:prepend(...) end

---@param ... Node|string Append script to execute with this element as context
function Element:executeAppend(...) end

---@class (exact) Text: Node
---@field type 'element'
---@field content string
local Text = {}

---@param prefix string
---@param name string
---@param attrs table<string, string>?
---@return Element
function mod.xml.element(prefix, name, attrs) end

---@param name string
---@param attrs table<string, string>?
---@return Element
function mod.xml.element(name, attrs) end
