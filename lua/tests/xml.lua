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

local function collect_attrsiter(iter)
  local result = {}
  for name, value in iter do
    result[name] = value
  end
  return result
end

local attrs = collect_attrsiter(second:attrs())
mod.debug.assert_equal(
  attrs,
  { a2 = 10, c = "hi", pi = 3.14 }
)

second.attrs.c = nil

local attrs = collect_attrsiter(second:rawattrs())
mod.debug.assert_equal(
  attrs,
  { a2 = "10", pi = "3.14" }
)

mod.debug._assert_throws(
  function() second.attrs() end
)

mod.debug._assert_throws(
  function() second.rawattrs() end
)

local added = mod.xml.element("hello")
added.attrs.b0 = false
added.attrs.b1 = true
added.attrs.f0 = 0.00000000000000000000008
added.attrs.f1 = 623453000000000000000000000000000
added.attrs.i0 = 1152921504606846976
added.attrs.f2 = 1152921504606846976.1
added.attrs.s0 = "hello"
added.attrs.s1 = "12345.78µあ"
added.attrs.s2 = "false"

mod.debug.assert_equal(
    collect_attrsiter(added:rawattrs()),
    {
      ["b0"] = "false",
      ["b1"] = "true",
      ["f0"] = "8e-23",
      ["f1"] = "6.23453e32",
      ["f2"] = "1.152921504606847e18",
      ["i0"] = "1152921504606846976",
      ["s0"] = "hello",
      ["s1"] = "12345.78µあ",
      ["s2"] = "false"
    }
)

mod.debug.assert_equal(
    collect_attrsiter(added:attrs()),
    {
      ["b0"] = false,
      ["b1"] = true,
      ["f0"] = 8e-23,
      ["f1"] = 6.23453e32,
      ["f2"] = 1.152921504606847e18,
      ["i0"] = 1152921504606846976,
      ["s0"] = "hello",
      ["s1"] = "12345.78µあ",
      ["s2"] = false
    }
)
