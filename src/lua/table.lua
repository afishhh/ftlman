mod.table = {}

function mod.table.iter_array(array)
  local i = 0
  local n = #array
  return function()
    i = i + 1
    if i <= n then return array[i] end
  end
end

function mod.table.compare_arrays(first, second)
  local zipped = mod.iter.zip(mod.table.iter_array(first), mod.table.iter_array(second))
  for i, a, b in mod.iter.enumerate(zipped) do
    if a < b then
      return -i
    elseif a > b then
      return i
    end
  end

  local nfirst = #first
  local nsecond = #second
  if nfirst < nsecond then
    return -(nfirst + 1)
  elseif nfirst > nsecond then
    return nsecond + 1
  else
    return 0
  end
end
