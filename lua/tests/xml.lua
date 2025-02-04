local root = mod.xml.element("root")

local first = mod.xml.element("first")
local second = mod.xml.element("second", { ["a2"] = "10", ["pi"] = "3.14" })
local fourth = mod.xml.element("fourth", { ["active"] = "false" })
root:prepend(first, second)
root:append("third", fourth)

local function mapNode(node)
  local element = node:as("element")
  if element then return "<" .. element.name .. ">"
  else
    local text = node:as("text")
    return assert(text).content
  end
end

mod.debug.assert_equal(
  mod.iter.collect(mod.iter.map(root:childNodes(), mapNode)),
  { "<first>", "<second>", "third", "<fourth>" }
)

local hi = mod.xml.element("hi")
local guys = mod.xml.element("guys")
second:after("greeting:", hi)
fourth.previousSibling:before(guys, "!")

mod.debug.assert_equal(
  mod.iter.collect(mod.iter.map(root:childNodes(), mapNode)),
  { "<first>", "<second>", "greeting:", "<hi>", "<guys>", "!", "third", "<fourth>" }
)

mod.debug.assert_equal(
  { second.attrs.a2, second.attrs.pi },
  { 10, 3.14 }
)

mod.debug.assert_equal(
  fourth.attrs.active,
  false
)

mod.debug.assert_equal(
  { second.rawattrs.a2 },
  { "10" }
)

mod.debug.assert_equal(
  second.rawattrs.abc,
  nil
)

second.rawattrs.c = "hi"

local attrs = mod.iter._collectpack(second:attrs())
table.sort(attrs, function(a, b)
  return a[1] < b[1]
end)
mod.debug.assert_equal(
  attrs,
  { { "a2", 10 }, { "c", "hi" }, { "pi", 3.14 } }
)

second.attrs.c = nil

local attrs = mod.iter._collectpack(second:rawattrs())
table.sort(attrs, function(a, b)
  return a[1] < b[1]
end)
mod.debug.assert_equal(
  attrs,
  { { "a2", "10" }, { "pi", "3.14" } }
)

mod.debug._assert_throws(
  function() second.attrs() end
)

mod.debug._assert_throws(
  function() second.rawattrs() end
)
