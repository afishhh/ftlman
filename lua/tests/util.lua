function iterators()
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
end

function eval()
  mod.debug.assert_equal(
    mod.util.eval("hello * world", {
      env = {
        hello = 29,
        world = 32
      }
    }),
    29 * 32
  )

  mod.debug.assert_equal(
    mod.util.eval([[
      function fun(hello) return hello .. world end
      return fun(fun(fun("a")))
    ]], {
      env = {
        world = " b"
      }
    }),
    "a b b b"
  )

  a = 3
  b = 4
  mod.debug.assert_equal(mod.util.eval("a * b", {
    env = _ENV
  }), 12)

  mod.debug.assert_equal(mod.util.eval("abc = 12", {
    env = _ENV
  }), nil)
  mod.debug.assert_equal(abc, 12)

  local env1 = {}
  mod.debug.assert_equal(mod.util.eval("abc = 13", { env = env1 }), nil)
  mod.debug.assert_equal(env1.abc, 13)
  mod.debug.assert_equal(abc, 12)

  mod.debug._assert_throws(function()
    mod.util.eval("count_a + count_b", { env = {} })
  end)
end

iterators()
eval()
