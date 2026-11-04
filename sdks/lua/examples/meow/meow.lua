-- meow for Forge: modal editing in the spirit of meow (not Vim
-- emulation). Selection first: motions make or extend the region, and
-- commands act on it. own keys stay reachable through the keypad
-- (SPC), so nothing needs a chord.
--
-- Normal state ([N], box cursor), QWERTY layout:
--
--   h j k l   left, down, up, right (cancel the selection)
--   w         select the word at point      e / b  select to the next / previous word
--   x         select the line (again: one more line)
--   f / t     select to / till a character (reads it)
--   ;         swap the selection's ends     g      cancel the selection
--   d         kill the selection (or the character)
--   c         change: kill it and insert    y      copy    p  paste
--   i / a     insert before / after         o / O  open a line below / above
--   u         undo                          /      search    :  M-x
--   SPC       keypad: SPC x f is C-x C-f, SPC m f is M-f, SPC g x is C-M-x,
--             SPC <letter> is C-c <letter>, SPC SPC <key> a plain key
--
-- Insert state ([I], bar cursor): type as usual; ESC goes back to normal.
-- Special buffers (help, buffer list) keep their own keys.

local NORMAL, INSERT = "meow-normal-mode", "meow-insert-mode"

forge.mode.define_minor(NORMAL)
forge.mode.define_minor(INSERT)
forge.mode.set_lighter(NORMAL, "[N]")
forge.mode.set_lighter(INSERT, "[I]")

local enabled = true

local function special(buf)
  local mode = buf:major_mode()
  return mode == "special-mode" or mode:match("^help") or mode:match("menu") or mode:match("occur")
    or mode:match("compilation") or mode:match("grep") or buf:name():match("^%*")
end

local function normal(buf)
  buf = buf or forge.buffer.current()
  buf:disable_minor_mode(INSERT)
  buf:enable_minor_mode(NORMAL)
  buf:set_option("cursor", "box")
end

local function insert(buf)
  buf = buf or forge.buffer.current()
  buf:disable_minor_mode(NORMAL)
  buf:enable_minor_mode(INSERT)
  buf:set_option("cursor", "bar")
end

local function off(buf)
  buf:disable_minor_mode(NORMAL)
  buf:disable_minor_mode(INSERT)
  buf:set_option("cursor", "")
end

-- ---------------------------------------------------------- selection

local function cur() return forge.buffer.current() end

local function has_selection(b)
  return b:region() ~= nil
end

local function symbol_bounds(line, col)
  local t = chars(line)
  local e = math.min(col, #t)
  if not t[e + 1] or not is_symbol(t[e + 1]) then
    while e + 1 <= #t and not is_symbol(t[e + 1]) do e = e + 1 end
  end
  local s = e + 1
  while s > 1 and is_symbol(t[s - 1]) do s = s - 1 end
  while e + 1 <= #t and is_symbol(t[e + 1]) do e = e + 1 end
  return s - 1, e
end

local function cancel(b)
  b:set_mark()
end

-- Character helpers (columns count characters, not bytes).
local function chars(s)
  local t = {}
  for _, c in utf8.codes(s) do t[#t + 1] = utf8.char(c) end
  return t
end

local function is_word(c) return c ~= nil and c:match("[%w_]") ~= nil end
local function is_symbol(c) return c ~= nil and (c:match("[%w_-]") ~= nil or c == "_") end

local command = function(name, doc, fn)
  forge.command.create(name, fn, { doc = doc })
end

-- h j k l: move, dropping the selection.
for key, spec in pairs({ h = { "meow-left", "backward-char" }, j = { "meow-next", "next-line" },
                         k = { "meow-prev", "previous-line" }, l = { "meow-right", "forward-char" } }) do
  command(spec[1], "Move (" .. spec[2] .. "), cancelling the selection.", function(n)
    cancel(cur())
    for _ = 1, (n or 1) do forge.command.run(spec[2]) end
  end)
  forge.keymap.set(key, spec[1], { mode = NORMAL })
end

command("meow-mark-word", "Select the word at point.", function()
  local b = cur()
  local line, col = b:point()
  local t = chars(b:line(line))
  local s, e = col, col
  if not is_word(t[col + 1]) then
    -- Not on a word: the next one on the line.
    while e < #t and not is_word(t[e + 1]) do e = e + 1 end
    s = e
  end
  while s > 0 and is_word(t[s]) do s = s - 1 end
  while e < #t and is_word(t[e + 1]) do e = e + 1 end
  b:set_mark(line, s)
  b:set_point(line, e)
end)

command("meow-next-word", "Select to the end of the next word (extending a selection).", function(n)
  local b = cur()
  if not has_selection(b) then b:set_mark(b:point()) end
  for _ = 1, (n or 1) do forge.command.run("forward-word") end
end)

command("meow-back-word", "Select to the start of the previous word (extending a selection).", function(n)
  local b = cur()
  if not has_selection(b) then b:set_mark(b:point()) end
  for _ = 1, (n or 1) do forge.command.run("backward-word") end
end)

command("meow-line", "Select the line; again, one more line.", function(n)
  local b = cur()
  local line = b:point()
  local l1, c1, l2 = b:region()
  if l1 and c1 == 0 then
    -- Extend the line selection downward.
    local last = math.min(l2 + (n or 1), b:line_count())
    b:set_point(last, #chars(b:line(last)))
  else
    b:set_mark(line, 0)
    local last = math.min(line + (n or 1) - 1, b:line_count())
    b:set_point(last, #chars(b:line(last)))
  end
end)

-- f / t: read a character, select up to it (f: including it).
local function find(include)
  return function()
    forge.ui.read_key(include and "Find: " or "Till: ", function(key)
      if not key then return end
      if key == "SPC" then key = " " end
      if utf8.len(key) ~= 1 then return forge.notify("Not a character: " .. key) end
      local b = cur()
      local line, col = b:point()
      for l = line, b:line_count() do
        local t = chars(b:line(l))
        local from = (l == line) and col + 2 or 1
        for i = from, #t do
          if t[i] == key then
            if not has_selection(b) then b:set_mark(line, col) end
            b:set_point(l, include and i or i - 1)
            return
          end
        end
      end
      forge.notify("No " .. key .. " after point")
    end)
  end
end
command("meow-find", "Select to the next occurrence of a character (read next), including it.", find(true))
command("meow-till", "Select up to the next occurrence of a character (read next).", find(false))

command("meow-reverse", "Swap the ends of the selection.", function()
  forge.command.run("exchange-point-and-mark")
end)

command("meow-cancel-selection", "Cancel the selection.", function() cancel(cur()) end)

command("meow-kill", "Kill the selection, or delete the character at point.", function()
  forge.command.run(has_selection(cur()) and "kill-region" or "delete-char")
end)

command("meow-change", "Kill the selection (or the character) and start inserting.", function()
  forge.command.run(has_selection(cur()) and "kill-region" or "delete-char")
  insert()
end)

command("meow-save", "Copy the selection.", function()
  if has_selection(cur()) then forge.command.run("kill-ring-save") else forge.notify("Nothing selected") end
end)

command("meow-yank", "Paste (yank).", function() forge.command.run("yank") end)
command("meow-undo", "Undo.", function() cancel(cur()); forge.command.run("undo") end)

command("meow-insert", "Insert before the selection (or at point).", function()
  local b = cur()
  local l1, c1 = b:region()
  if l1 then b:set_point(l1, c1) end
  cancel(b)
  insert(b)
end)

command("meow-append", "Insert after the selection (or after the character at point).", function()
  local b = cur()
  local l1, c1, l2, c2 = b:region()
  if l1 then
    b:set_point(l2, c2)
  else
    local line, col = b:point()
    if col < #chars(b:line(line)) then b:set_point(line, col + 1) end
  end
  cancel(b)
  insert(b)
end)

command("meow-open-below", "Open a line below and insert there.", function()
  local b = cur()
  cancel(b)
  local line = b:point()
  b:set_point(line, #chars(b:line(line)))
  b:insert("\n")
  b:indent_line(line + 1)
  insert(b)
end)

command("meow-open-above", "Open a line above and insert there.", function()
  local b = cur()
  cancel(b)
  local line = b:point()
  b:set_point(line, 0)
  b:insert("\n")
  b:set_point(line, 0)
  b:indent_line(line)
  insert(b)
end)

command("meow-insert-exit", "Back to normal state.", function() normal() end)

command("meow-undefined", "Keys normal state doesn't use (they don't insert).", function()
  forge.notify("Not bound in normal state (i to insert, SPC for the keypad)")
end)

-- ------------------------------------------------------------- keypad

-- Keys typed after SPC, turned into a key sequence: the first key
-- picks the prefix (x: C-x, c: C-c, h: C-h, m: Meta, g: C-M-, else
-- C-c <key>); later keys get Control, unless they come after SPC.
local function keypad_step(seq, literal, modifier)
  forge.ui.read_key("SPC " .. table.concat(seq, " ") .. (modifier and (" " .. modifier) or ""), function(key)
    if not key then return end
    if key == "SPC" and not literal and not modifier and #seq > 0 then
      return keypad_step(seq, true, nil)
    end
    local k
    if #seq == 0 and not modifier then
      local first = { x = "C-x", c = "C-c", h = "C-h" }
      if first[key] then
        k = first[key]
      elseif key == "m" then
        return keypad_step(seq, false, "M-")
      elseif key == "g" then
        return keypad_step(seq, false, "C-M-")
      else
        seq[1] = "C-c"
        k = key
      end
    elseif modifier then
      k = modifier .. key
    elseif literal or key:match("^[CM]%-") or #key > 1 then
      k = key
    else
      k = "C-" .. key
    end
    seq[#seq + 1] = k
    local keys = table.concat(seq, " ")
    local found = forge.keymap.get(keys)
    if not found and not forge.keymap.is_prefix(keys) and k == "C-" .. key then
      -- C-x f isn't C-x C-f: try the plain key.
      seq[#seq] = key
      keys = table.concat(seq, " ")
      found = forge.keymap.get(keys)
    end
    if found then
      forge.keymap.execute(keys)
    elseif forge.keymap.is_prefix(keys) then
      keypad_step(seq, false, nil)
    else
      forge.notify(keys .. " is undefined")
    end
  end)
end

command("meow-keypad", "Type a key sequence without chords: SPC x f is C-x C-f.", function()
  keypad_step({}, false, nil)
end)

-- ------------------------------------------------------------ keys

local NORMAL_KEYS = {
  w = "meow-mark-word", e = "meow-next-word", b = "meow-back-word", x = "meow-line",
  f = "meow-find", t = "meow-till", [";"] = "meow-reverse", g = "meow-cancel-selection",
  d = "meow-kill", c = "meow-change", y = "meow-save", p = "meow-yank", u = "meow-undo",
  i = "meow-insert", a = "meow-append", o = "meow-open-below", O = "meow-open-above",
  ["/"] = "isearch-forward", [":"] = "execute-extended-command", SPC = "meow-keypad",
  ESC = "meow-cancel-selection",
}

-- Every other printable key does nothing in normal state (it mustn't insert).
for code = 33, 126 do
  local k = string.char(code)
  if not NORMAL_KEYS[k] and not ("hjkl"):find(k, 1, true) then
    forge.keymap.set(k, "meow-undefined", { mode = NORMAL })
  end
end
for k, cmd in pairs(NORMAL_KEYS) do
  forge.keymap.set(k, cmd, { mode = NORMAL })
end
forge.keymap.set("ESC", "meow-insert-exit", { mode = INSERT })

-- ------------------------------------------------------------ states

local function setup(buf)
  if enabled and buf:alive() and not special(buf) then
    normal(buf)
  end
end

forge.on("buffer-opened", setup)

command("meow-mode", "Turn modal editing on or off everywhere.", function()
  enabled = not enabled
  for _, b in ipairs(forge.buffer.all()) do
    if enabled then setup(b) else off(b) end
  end
  forge.notify("Meow mode " .. (enabled and "enabled" or "disabled"))
end)

command("meow-normal-state", "Switch this buffer to normal state.", function() normal() end)
command("meow-insert-state", "Switch this buffer to insert state.", function() insert() end)

for _, b in ipairs(forge.buffer.all()) do setup(b) end
