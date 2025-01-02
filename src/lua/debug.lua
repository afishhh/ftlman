mod.debug = {}

function mod.debug.__shallow_stringify_array(array)
  local result = "{"
  for i, value in ipairs(array) do
    if i > 1 then
      result = result .. ", "
    end
    result = result .. tostring(value)
  end
  result = result .. "}"
  return result
end

function mod.debug.assert_equal(a, b)
  if type(a) ~= type(b) then
    local message = "assertion failed: mismatched types"
    message = message .. "\nlhs = " .. type(a)
    message = message .. "\nrhs = " .. type(b)
    error(message)
  end

  if type(a) == "table" and type(b) == "table" then
    -- TODO: Make this work with whole tables not arrays
    local index = math.abs(mod.table.compare_arrays(a, b))
    if index ~= 0 then
      local message = "assertion failed: differing tables"
      message = message .. "\nlhs = " .. mod.debug.__shallow_stringify_array(a)
      message = message .. "\nrhs = " .. mod.debug.__shallow_stringify_array(b)
      message = message .. "\nmismatch occured at index " .. tostring(index)
      error(message)
    end
  elseif a == nil and b == nil then
  else
    error("unimplemented")
  end
end
