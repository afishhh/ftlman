mod.util = {}

function mod.util.readonly(table)
  -- https://www.lua.org/pil/13.4.5.html
  local proxy = {}
  local mt = {
    __index = table,
    __newindex = function (t,k,v)
      error("attempt to update a read-only table", 2)
    end
  }
  setmetatable(proxy, mt)
  return proxy
end
