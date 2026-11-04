-- An example Forge plugin in Lua. It runs inside a WebAssembly component,
-- sandboxed like plugins in any other language: the forge API is the
-- same as init.rhai's, minus what a plugin isn't allowed to do.
--
--   ../../build.sh plugin.lua -o hello-lua.wasm
--   forge --plugin hello-lua.wasm        (then M-x hello-lua-greet)

forge.command.create("hello-lua-greet", function(n)
  forge.ui.input({ prompt = "Greet whom? ", default = "world" }, function(name)
    if name then
      forge.buffer.current():insert(string.rep("Hello, " .. name .. "!\n", n or 1))
    end
  end)
end, { doc = "Ask for a name and insert a greeting at point." })

forge.command.create("hello-lua-count-words", function()
  local _, words = forge.buffer.current():text():gsub("%S+", "")
  forge.notify(words .. " words")
end, { doc = "Show how many words the buffer has." })

-- Lua errors become command errors (with a traceback in *Messages*);
-- pcall catches them as usual.
forge.command.create("hello-lua-fail", function()
  local ok, e = pcall(error, "caught")
  assert(not ok and e == "caught")
  error("this command always fails")
end, { doc = "Fail on purpose." })

forge.on("buffer-saved", function(buf)
  print("saved " .. buf:name())
end)

print("Hello from Lua " .. _VERSION .. "!")

-- Running a program (needs the processes permission in forge.toml): the
-- output arrives later, when the editor is idle.
forge.command.create("hello-lua-wc", function()
  local out = ""
  local id, err = forge.process.spawn("wc", { "-w" }, {
    on_output = function(data) out = out .. data end,
    on_exit = function(code) forge.notify("wc: " .. out:gsub("%s", "") .. " words (exit " .. tostring(code) .. ")") end,
  })
  if not id then error(err) end
  forge.process.write(id, forge.buffer.current():text())
  forge.process.close_input(id)
end, { doc = "Count words with wc, as an example of running a program." })

-- A background command: it runs on a snapshot of the editor, so a slow
-- one doesn't freeze typing. Its edits are merged in when it's done.
forge.command.create("hello-lua-slow-upcase", function()
  local buf = forge.buffer.current()
  local n = 0
  for i = 1, 2000000 do n = n + i % 7 end -- pretend to think hard
  local text = buf:text()
  buf:replace(1, 0, buf:line_count(), #buf:line(buf:line_count()), text:upper())
  print("upcased (" .. n .. ")")
end, { doc = "Upcase the buffer, slowly, in the background.", background = true })
