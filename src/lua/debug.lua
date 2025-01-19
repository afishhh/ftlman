mod.debug = {}

function mod.debug.assert_equal(a, b)
  if type(a) ~= type(b) then
    local message = "assertion failed: mismatched types"
    message = message .. "\nlhs = " .. type(a)
    message = message .. "\nrhs = " .. type(b)
    error(message)
  end

  local function fail()
      local message = "assertion failed: lhs does not match rhs"
      message = message .. "\nlhs = " .. mod.debug.pretty_string(a)
      message = message .. "\nrhs = " .. mod.debug.pretty_string(b)
      error(message)
  end

  local equal = mod.debug._compare(a, b)
  if not equal then
    fail()
  end
end

function mod.debug._assert_throws(fun)
  local success = pcall(fun)
  if success then
    error("assertion failed: function didn't throw")
  end
end
