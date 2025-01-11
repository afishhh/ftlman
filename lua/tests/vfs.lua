local fs = mod.vfs.files;

local function compare_dirent(a, b)
  return a.filename < b.filename
end

local root_files = fs:ls("/")
table.sort(root_files, compare_dirent)
mod.debug.assert_equal(root_files, {
  { ["filename"] = "abc.txt", ["type"] = "file" },
  { ["filename"] = "dir", ["type"] = "dir" },
})

local dir_files = fs:ls("/dir")
table.sort(dir_files, compare_dirent)
mod.debug.assert_equal(dir_files, {
  { ["filename"] = "1k", ["type"] = "file" },
  { ["filename"] = "one", ["type"] = "file" },
  { ["filename"] = "three", ["type"] = "dir" },
  { ["filename"] = "two", ["type"] = "file" },
})

local a = fs:read("/dir/1k")
mod.debug.assert_equal(a, string.rep("a", 1000) .. "\n")

mod.debug.assert_equal(fs:stat("/dir/three/doesnotexist"), nil)
mod.debug.assert_equal(fs:stat("/dir/1k"), { ["type"] = "file", ["length"] = 1001 })
mod.debug.assert_equal(fs:stat("/dir/two"), { ["type"] = "file", ["length"] = 2 })
mod.debug.assert_equal(fs:stat("/dir/three"), { ["type"] = "dir" })

mod.debug._assert_throws(function()
  fs:read("/dir/three/doesnotexist")
end)
-- not supported by LuaDirectoryFS
mod.debug._assert_throws(function()
  fs:write("/anything")
end)
