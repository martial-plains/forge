-- A Language Server Protocol client for Forge, as a Lua plugin.
--
-- When a file opens in a mode with a known server, the server for that
-- project starts (one per server and project root) and gets the file.
-- Its diagnostics become decorations (the message shows when point is on
-- them); M-. goes to a definition, C-h . shows what's at point (hover).
--
-- Servers run as processes (the processes permission in forge.toml names
-- them). Everything is asynchronous: requests go out, replies arrive
-- through on_output when the editor is idle.

local SERVERS = {
  ["python-mode"] = { cmd = "pyright-langserver", args = { "--stdio" }, language = "python" },
  ["rust-mode"] = { cmd = "rust-analyzer", args = {}, language = "rust" },
  ["c-mode"] = { cmd = "clangd", args = {}, language = "c" },
  ["go-mode"] = { cmd = "gopls", args = {}, language = "go" },
  ["js-mode"] = { cmd = "typescript-language-server", args = { "--stdio" }, language = "javascript" },
  ["sh-mode"] = { cmd = "bash-language-server", args = { "start" }, language = "shellscript" },
  ["lua-mode"] = { cmd = "lua-language-server", args = {}, language = "lua" },
}

local servers = {}   -- key (cmd .. root) -> server
local docs = {}      -- buffer id -> { server =, uri =, version = }
local failed = {}    -- cmd -> error (don't retry every time)

-- ------------------------------------------------------------ helpers

local function uri_of(path)
  return "file://" .. path:gsub("[^%w/%-._~]", function(c) return string.format("%%%02X", c:byte()) end)
end

local function path_of(uri)
  local p = uri:gsub("^file://", "")
  return (p:gsub("%%(%x%x)", function(h) return string.char(tonumber(h, 16)) end))
end

local function dirname(path)
  return path:match("^(.*)/[^/]*$") or "."
end

-- LSP columns count UTF-16 code units; Forge's count characters.
local function utf16_col(line, col)
  local units, n = 0, 0
  for _, c in utf8.codes(line) do
    if n >= col then break end
    units = units + (c >= 0x10000 and 2 or 1)
    n = n + 1
  end
  return units
end

local function char_col(line, units)
  local u, n = 0, 0
  for _, c in utf8.codes(line) do
    if u >= units then break end
    u = u + (c >= 0x10000 and 2 or 1)
    n = n + 1
  end
  return n
end

local function lsp_position(buf)
  local line, col = buf:point()
  return { line = line - 1, character = utf16_col(buf:line(line), col) }
end

-- --------------------------------------------------------- messaging

local function send(server, msg)
  msg.jsonrpc = "2.0"
  local body = json.encode(msg)
  local ok, err = pcall(forge.process.write, server.process, "Content-Length: " .. #body .. "\r\n\r\n" .. body)
  if not ok then forge.log("warn", server.cmd .. ": " .. tostring(err)) end
end

-- Notifications wait until the server is initialized.
local function notify(server, method, params)
  local msg = { method = method, params = params }
  if server.ready then send(server, msg) else table.insert(server.queue, msg) end
end

local function request(server, method, params, on_result)
  server.next_id = server.next_id + 1
  local id = server.next_id
  server.pending[id] = on_result
  local msg = { id = id, method = method, params = params }
  if server.ready or method == "initialize" then send(server, msg) else table.insert(server.queue, msg) end
end

local function buffer_for_uri(uri)
  for id, d in pairs(docs) do
    if d.uri == uri then return forge.buffer.from_id(id) end
  end
end

local SEVERITY = { "error", "warning", "info", "info" }

local function publish_diagnostics(params)
  local buf = buffer_for_uri(params.uri)
  if not buf or not buf:alive() then return end
  local decos = {}
  for _, d in ipairs(params.diagnostics) do
    local s, e = d.range.start, d.range["end"]
    local l1, l2 = s.line + 1, e.line + 1
    local c1 = char_col(buf:line(l1), s.character)
    local c2 = char_col(buf:line(l2), e.character)
    if l1 == l2 and c2 <= c1 then c2 = c1 + 1 end -- a point: mark one character
    local source = (type(d.source) == "string") and (d.source .. ": ") or ""
    local msg = source .. tostring(d.message):gsub("\n.*", "")
    decos[#decos + 1] = { l1, c1, l2, c2, SEVERITY[d.severity] or "error", msg }
  end
  buf:set_decorations(decos)
  local errors = 0
  for _, d in ipairs(decos) do if d[5] == "error" then errors = errors + 1 end end
  forge.log("debug", string.format("%s: %d diagnostics (%d errors)", buf:name(), #decos, errors))
end

local function on_server_request(server, msg)
  local result = json.null
  if msg.method == "workspace/configuration" then
    result = json.array()
    for i = 1, #(msg.params.items or {}) do result[i] = json.null end
  elseif msg.method == "client/registerCapability" or msg.method == "window/workDoneProgress/create"
      or msg.method == "client/unregisterCapability" then
    result = json.null
  elseif msg.method == "workspace/workspaceFolders" then
    result = json.array({ { uri = uri_of(server.root), name = server.root:match("[^/]*$") } })
  else
    send(server, { id = msg.id, error = { code = -32601, message = "not supported: " .. msg.method } })
    return
  end
  send(server, { id = msg.id, result = result })
end

local function handle(server, msg)
  if msg.method and msg.id ~= nil then
    on_server_request(server, msg)
  elseif msg.method then
    if msg.method == "textDocument/publishDiagnostics" then
      publish_diagnostics(msg.params)
    elseif msg.method == "window/showMessage" and (msg.params.type or 4) <= 2 then
      forge.notify(server.cmd .. ": " .. msg.params.message)
    elseif msg.method == "window/logMessage" then
      forge.log("debug", server.cmd .. ": " .. tostring(msg.params.message))
    end
  elseif msg.id ~= nil then
    local cb = server.pending[msg.id]
    server.pending[msg.id] = nil
    if msg.error and msg.error ~= json.null then
      forge.notify(server.cmd .. ": " .. tostring(msg.error.message), "warn")
    elseif cb then
      cb(msg.result)
    end
  end
end

-- Content-Length framed messages out of the byte stream.
local function on_output(server, data)
  server.input = server.input .. data
  while true do
    local header_end = server.input:find("\r\n\r\n", 1, true)
    if not header_end then return end
    local length = tonumber(server.input:sub(1, header_end):match("[Cc]ontent%-[Ll]ength:%s*(%d+)"))
    if not length then
      server.input = server.input:sub(header_end + 4)
    else
      local body_start = header_end + 4
      if #server.input < body_start + length - 1 then return end
      local body = server.input:sub(body_start, body_start + length - 1)
      server.input = server.input:sub(body_start + length)
      local ok, msg = pcall(json.decode, body)
      if ok then
        local ok2, err = pcall(handle, server, msg)
        if not ok2 then forge.log("error", "lsp: " .. tostring(err)) end
      else
        forge.log("warn", server.cmd .. ": bad message: " .. tostring(msg))
      end
    end
  end
end

-- ------------------------------------------------------------ servers

local function start(spec, root)
  local key = spec.cmd .. "\0" .. root
  if servers[key] then return servers[key] end
  if failed[spec.cmd] then return nil end
  local server = { cmd = spec.cmd, root = root, key = key, next_id = 0, pending = {}, queue = {}, input = "", ready = false }
  local id, err = forge.process.spawn(spec.cmd, spec.args, {
    dir = root,
    on_output = function(data) on_output(server, data) end,
    on_exit = function(code)
      servers[key] = nil
      for bid, d in pairs(docs) do
        if d.server == server then docs[bid] = nil end
      end
      if not server.stopping then
        forge.notify(spec.cmd .. " exited" .. (code and (" with code " .. code) or ""), "warn")
      end
    end,
  })
  if not id then
    failed[spec.cmd] = err
    forge.log("warn", "lsp: can't start " .. spec.cmd .. ": " .. tostring(err))
    return nil
  end
  server.process = id
  servers[key] = server
  request(server, "initialize", {
    processId = json.null,
    rootUri = uri_of(root),
    rootPath = root,
    workspaceFolders = json.array({ { uri = uri_of(root), name = root:match("[^/]*$") } }),
    clientInfo = { name = "forge" },
    capabilities = {
      general = { positionEncodings = json.array({ "utf-16" }) },
      workspace = { configuration = true, workspaceFolders = true },
      textDocument = {
        synchronization = { didSave = true, dynamicRegistration = false },
        publishDiagnostics = { relatedInformation = false },
        hover = { contentFormat = json.array({ "plaintext", "markdown" }) },
        definition = { linkSupport = false },
        completion = { completionItem = { snippetSupport = false } },
      },
    },
  }, function()
    send(server, { method = "initialized", params = {} })
    server.ready = true
    for _, msg in ipairs(server.queue) do send(server, msg) end
    server.queue = {}
    forge.log("info", spec.cmd .. " started for " .. root)
  end)
  return server
end

local function attach(buf)
  if not buf:alive() or docs[buf:id()] then return end
  local path = buf:path()
  local spec = SERVERS[buf:major_mode()]
  if not path or not spec then return end
  local root = forge.process.project_root(path) or dirname(path)
  local server = start(spec, root)
  if not server then return end
  local doc = { server = server, uri = uri_of(path), version = 1, synced = buf:version() }
  docs[buf:id()] = doc
  notify(server, "textDocument/didOpen", {
    textDocument = { uri = doc.uri, languageId = spec.language, version = doc.version, text = buf:text() },
  })
end

local function detach(id)
  local doc = docs[id]
  if not doc then return end
  docs[id] = nil
  notify(doc.server, "textDocument/didClose", { textDocument = { uri = doc.uri } })
end

-- ------------------------------------------------------------- events

forge.on("buffer-opened", attach)
forge.on("mode-changed", function(buf)
  local doc = docs[buf:id()]
  if doc and not SERVERS[buf:major_mode()] then detach(buf:id()) end
  attach(buf)
end)
forge.on("buffer-changed", function(buf)
  local doc = docs[buf:id()]
  if not doc then return end
  if doc.synced == buf:version() then return end
  doc.version = doc.version + 1
  doc.synced = buf:version()
  notify(doc.server, "textDocument/didChange", {
    textDocument = { uri = doc.uri, version = doc.version },
    contentChanges = json.array({ { text = buf:text() } }),
  })
end)
forge.on("buffer-saved", function(buf)
  local doc = docs[buf:id()]
  if doc then notify(doc.server, "textDocument/didSave", { textDocument = { uri = doc.uri } }) end
end)
forge.on("buffer-killed", function(buf) detach(buf:id()) end)

-- ----------------------------------------------------------- commands

local function current_doc()
  local buf = forge.buffer.current()
  local doc = docs[buf:id()]
  if not doc then
    error("no language server for this buffer (" .. buf:major_mode() .. ")", 0)
  end
  return buf, doc
end

local function hover_text(contents)
  if type(contents) == "string" then return contents end
  if contents.value then return contents.value end
  local parts = {}
  for _, c in ipairs(contents) do parts[#parts + 1] = type(c) == "string" and c or c.value end
  return table.concat(parts, "\n")
end

-- Completion: a source for the completion popup (corfu) and M-TAB.
local KINDS = { "text", "method", "function", "constructor", "field", "variable", "class", "interface",
  "module", "property", "unit", "value", "enum", "keyword", "snippet", "color", "file", "reference",
  "folder", "enum member", "constant", "struct", "event", "operator", "type parameter" }

local function sync(buf, doc)
  -- Changes reach the server when the editor is idle; completion can
  -- come first.
  if doc.synced ~= buf:version() then
    doc.version = doc.version + 1
    doc.synced = buf:version()
    notify(doc.server, "textDocument/didChange", {
      textDocument = { uri = doc.uri, version = doc.version },
      contentChanges = json.array({ { text = buf:text() } }),
    })
  end
end

local source_modes = {}
for mode in pairs(SERVERS) do source_modes[#source_modes + 1] = mode end

forge.completion.source("lsp", function(buf, line, col, reply)
  local doc = docs[buf:id()]
  if not doc then return reply(line, col, line, col, {}) end
  sync(buf, doc)
  local text = buf:line(line)
  -- The word before point (as Forge sees it: letters, digits, _).
  local t = {}
  for _, c in utf8.codes(text) do t[#t + 1] = utf8.char(c) end
  local start = col
  while start > 0 and t[start]:match("[%w_]") do start = start - 1 end
  request(doc.server, "textDocument/completion", {
    textDocument = { uri = doc.uri },
    position = { line = line - 1, character = utf16_col(text, col) },
  }, function(result)
    local items = {}
    if result ~= json.null and result then
      local list = result.items or result
      table.sort(list, function(a, b) return (a.sortText or a.label) < (b.sortText or b.label) end)
      for i, item in ipairs(list) do
        if i > 200 then break end
        local word = item.insertText
        if not word or item.insertTextFormat == 2 or word:find("%$") then
          word = (item.filterText or item.label):gsub("%(.*$", "")
        end
        word = word:gsub("^%s+", "")
        local note = item.detail
        if note == json.null or not note or note == "" then note = KINDS[item.kind] end
        if type(note) == "string" and #note > 40 then note = note:sub(1, 37) .. "..." end
        items[#items + 1] = { text = word, annotation = note }
      end
    end
    reply(line, start, line, col, items)
  end)
end, { modes = source_modes })

forge.command.create("lsp-hover", function()
  local buf, doc = current_doc()
  request(doc.server, "textDocument/hover", {
    textDocument = { uri = doc.uri }, position = lsp_position(buf),
  }, function(result)
    if result == json.null or not result.contents then
      forge.notify("Nothing here")
      return
    end
    local lines = {}
    for l in hover_text(result.contents):gmatch("[^\n]+") do
      if not l:match("^```") and l:match("%S") then lines[#lines + 1] = l end
      if #lines == 3 then break end
    end
    forge.notify(table.concat(lines, "  "))
  end)
end, { doc = "Show what the language server says about the thing at point." })

forge.command.create("lsp-find-definition", function()
  local buf, doc = current_doc()
  request(doc.server, "textDocument/definition", {
    textDocument = { uri = doc.uri }, position = lsp_position(buf),
  }, function(result)
    if result == json.null then return forge.notify("No definition found") end
    local loc = result.uri and result or result[1]
    if not loc then return forge.notify("No definition found") end
    local uri, range = loc.uri or loc.targetUri, loc.range or loc.targetSelectionRange
    local target, err = forge.buffer.visit(path_of(uri))
    if not target then return forge.notify(err, "warn") end
    target:show()
    local line = range.start.line + 1
    target:set_point(line, char_col(target:line(line), range.start.character))
    attach(target)
  end)
end, { doc = "Go to the definition of the thing at point (from the language server)." })

forge.command.create("lsp-restart", function()
  for _, s in pairs(servers) do
    s.stopping = true
    forge.process.kill(s.process)
  end
  servers, docs, failed = {}, {}, {}
  for _, b in ipairs(forge.buffer.all()) do attach(b) end
  forge.notify("Restarting language servers")
end, { doc = "Stop the language servers and start them again for the open files." })

forge.command.create("lsp-servers", function()
  local lines = {}
  for _, s in pairs(servers) do
    local n = 0
    for _, d in pairs(docs) do if d.server == s then n = n + 1 end end
    lines[#lines + 1] = string.format("%s in %s: %d file%s%s", s.cmd, s.root, n, n == 1 and "" or "s", s.ready and "" or " (starting)")
  end
  forge.notify(#lines > 0 and table.concat(lines, "; ") or "No language servers running")
end, { doc = "Show which language servers run, and for which projects." })

pcall(forge.keymap.set, "M-.", "lsp-find-definition")
pcall(forge.keymap.set, "C-h .", "lsp-hover")

-- Files already open when the plugin starts.
for _, b in ipairs(forge.buffer.all()) do attach(b) end
