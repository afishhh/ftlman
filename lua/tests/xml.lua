local root = mod.xml.element("root")

local first = mod.xml.element("first")
local second = mod.xml.element("second")
local fourth = mod.xml.element("fourth")
root:prepend(first, second)
root:append("third", fourth)

local mapped =  mod.iter.map(root:childNodes(), function(node)
  local element = node:as("element")
  if element then return "<" .. element.name .. ">"
  else
    local text = node:as("text")
    return assert(text).content
  end
end)

mod.debug.assert_equal(
  mod.iter.collect(mapped),
  { "<first>", "<second>", "third", "<fourth>" }
)
