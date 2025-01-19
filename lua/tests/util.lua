local a = {"a", "b", "c"}
local b = {"b", "d", 7.5, "e"}
local it = mod.iter.enumerate(
  mod.iter.zip(
    mod.table.iter_array(a),
    mod.table.iter_array(b)
  ),
  3
)
mod.debug.assert_equal(table.pack(it()), {3, "a", "b", ["n"] = 3})
mod.debug.assert_equal(table.pack(it()), {4,  "b", "d", ["n"] = 3})
mod.debug.assert_equal(table.pack(it()), {5, "c", 7.5, ["n"] = 3})
mod.debug.assert_equal(it(), nil)

local count = mod.iter.count(mod.iter.zip(
    mod.table.iter_array(a),
    mod.table.iter_array(b)
))
local count_a = mod.iter.count(mod.table.iter_array(a))
local count_b = mod.iter.count(mod.table.iter_array(b))

mod.debug.assert_equal(count, math.min(count_a, count_b))
mod.debug.assert_equal(count_a, 3)
mod.debug.assert_equal(count_b, 4)
