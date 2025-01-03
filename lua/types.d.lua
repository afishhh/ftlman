---@meta

---@class _ModApi
mod = {
  xml = {},
  util = {},
}

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
local Node = {}

---@param type 'element'
---@return Element?
function Node:as(type) end

---@param type 'text'
---@return Text?
function Node:as(type) end

---@class (exact) Element: Node
---@field type 'element'
---@field name 'string'
---@field prefix 'string'
---@field textContent string
local Element = {}

---@return fun(): Element
function Element:children() end

---@return fun(): Node
function Element:childNodes() end

---@param ... Node|string
function Element:append(...) end

---@class (exact) Text: Node
---@field type 'element'
---@field content string
local Text = {}

---@generic T: table
---@return T
function mod.util.readonly(tbl) end

---@generic T
---@param iterator fun(): T?
---@param start? number
---@return fun(): integer, T?
function mod.util.enumerate(iterator, start) end

---@param prefix string
---@param name string
---@param attrs table<string, string>?
---@return Element
function mod.xml.element(prefix, name, attrs) end

---@param name string
---@param attrs table<string, string>?
---@return Element
function mod.xml.element(name, attrs) end
