function mod.util.enumerate(iterator, start)
  i = start or 1
  return function()
    next = iterator()
    if next ~= nil then
      tmp = i
      i = i + 1
      return tmp, next
    end
  end
end
