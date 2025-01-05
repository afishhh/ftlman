
local a = {"a", "b", "c"}
local b = {"b", "d", "f", "e"}
local it = mod.iter.enumerate(
  mod.iter.zip(
    mod.table.iter_array(a),
    mod.table.iter_array(b)
  ),
  3
)
mod.debug.assert_equal(table.pack(it()), {3, "a", "b"})
mod.debug.assert_equal(table.pack(it()), {4,  "b", "d"})
mod.debug.assert_equal(table.pack(it()), {5, "c", "f"})
mod.debug.assert_equal(it(), nil)

