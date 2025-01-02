mod.iter = {}

function mod.iter.enumerate(iterator, start)
  local i = start or 1
  return function()
    local values = table.pack(iterator())
    if #values > 0 then
      local tmp = i
      i = i + 1
      return tmp, table.unpack(values)
    end
  end
end

-- TODO: Don't assume fused iterators?
--       Have a fuse() adapter maybe
function mod.iter.zip(a, b)
  return function()
    na = table.pack(a())
    nb = table.pack(b())
    if #na == 0 or #nb == 0 then
      return nil
    end
    
    return table.unpack(na), table.unpack(nb)
  end
end

function mod.iter.collect(iterator)
  local result = {}
  for value in iterator do
    result[#result + 1] = value
  end
  return result
end

function mod.iter.count(iterator)
  local count = 0
  for value in iterator do
    count = count + 1
  end
  return count
end

function mod.iter.map(iterator, mapper)
  return function()
    next = iterator()
    if next ~= nil then
      return mapper(next)
    end
  end
end
