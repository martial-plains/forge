-- vertico for Forge: every prompt with candidates (M-x, C-x b, C-x C-f,
-- plugin questions) lists them vertically under the prompt, filtered as
-- you type. C-n / C-p (or the arrows) move, RET picks, TAB inserts the
-- selected candidate, M-RET takes the input as typed.
--
-- With it: orderless matching (space-separated parts in any order, a
-- part starting with ! excludes) and marginalia-style annotations: a
-- command's key and summary in M-x, a buffer's mode and size in C-x b.
--
-- Forge draws and filters the list (a plugin never runs on each key);
-- this plugin chooses how it looks and matches, and writes the
-- annotations once per list.

local LINES = 10
local STYLES = { "orderless", "flex" }

local function enable(on)
  forge.completion.set_minibuffer_ui({ vertical = on, lines = LINES })
  forge.completion.set_matching(on and STYLES or { "prefix" })
end

-- The first sentence of a docstring, short.
local function summary(doc)
  if not doc then return "" end
  local first = doc:match("^[^\n]*") or ""
  first = first:match("^(.-%.)%s") or first
  if #first > 60 then first = first:sub(1, 57) .. "..." end
  return first
end

forge.completion.annotate("command", function(names)
  local out = {}
  for i, name in ipairs(names) do
    local keys = forge.keymap.where_is(name)
    local key = keys[1] and ("(" .. keys[1] .. ")  ") or ""
    out[i] = key .. summary(forge.command.doc(name))
  end
  return out
end)

forge.completion.annotate("buffer", function(names)
  local out = {}
  for i, name in ipairs(names) do
    local b = forge.buffer.find(name)
    if b then
      local mode = b:major_mode():gsub("%-mode$", "")
      local lines = b:line_count()
      local where = b:path() or ""
      local home = where:match("^/home/[^/]+") or where:match("^/Users/[^/]+") or where:match("^/root")
      if home then where = "~" .. where:sub(#home + 1) end
      out[i] = string.format("%-12s %6d lines  %s%s", mode, lines, b:modified() and "* " or "", where)
    else
      out[i] = ""
    end
  end
  return out
end)

local on = true
enable(on)

forge.mode.define_minor("vertico-mode")
forge.mode.set_lighter("vertico-mode", "")

forge.command.create("vertico-mode", function()
  on = not on
  enable(on)
  forge.notify("Vertico mode " .. (on and "enabled" or "disabled"))
end, { doc = "Turn vertical minibuffer completion on or off." })
