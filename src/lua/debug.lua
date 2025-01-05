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


  if type(a) == "table" and type(b) == "table" then
    -- TODO: Make this work with whole tables not arrays
    local index = math.abs(mod.table.compare_arrays(a, b))
    if index ~= 0 then
      fail()
    end
  elseif a == nil and b == nil then
  else
    error("unimplemented")
  end
end
