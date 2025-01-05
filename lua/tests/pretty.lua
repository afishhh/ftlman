local test = {
  ["ten"] = 10,
  ["ten_point_one"] = 10.1,
  ["false"] = false,
  true,
  [mod.xml.element("root")] = {
    mod.xml.element("one"),
    function() end,
    mod.xml.element("three"),
    error,
  },
  [{1, 2, 3}] = 12,
}
test.cyclic = test

print(mod.debug.pretty_string(test, { colors = "ansi" }))
print(mod.debug.pretty_string(test, { colors = "ansi", indent = "\t" }))
print(mod.debug.pretty_string(test, { recursive = false, colors = "ansi" }))
print(mod.debug.pretty_string("hello world", { recursive = false, colors = "ansi" }))
