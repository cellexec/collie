-- collie: Composer für eine neue Nachricht an einen Chat.
--
-- collie startet `nvim -S collie-message.lua -- <datei>` in einem Popup. Das
-- Plugin gilt nur für den Puffer der Nachricht: Filetype `colliemsg`,
-- Hervorhebung und Vervollständigung für Slash-Befehle (`/`) und Dateien
-- (`@`) wie in Claude Code. `:wq` oder Ctrl-S schickt, `:q!` verwirft.

local buf = vim.api.nvim_get_current_buf()
local cwd = vim.fn.getcwd()
local to = vim.env.COLLIE_MESSAGE_TO or 'Chat'
local claude_home = vim.env.CLAUDE_CONFIG_DIR or ((vim.env.HOME or '~') .. '/.claude')
local MAX_ITEMS = 80
local MAX_FILES = 30000

vim.bo[buf].filetype = 'colliemsg'
vim.bo[buf].syntax = 'markdown'
-- Eigene Vervollständigung; blink.cmp und nvim-cmp schweigen hier.
vim.b[buf].completion = false
pcall(function()
  require('cmp').setup.buffer({ enabled = false })
end)
vim.opt_local.wrap = true
vim.opt_local.linebreak = true
if not pcall(function()
  vim.opt_local.completeopt = { 'menuone', 'noinsert', 'popup' }
end) then
  pcall(function()
    vim.opt_local.completeopt = { 'menuone', 'noinsert' }
  end)
end

vim.wo.winbar = ('%%#Title# 󰏫 Nachricht an %s%%*  %%#Comment#:wq oder Ctrl-S schickt · :q! verwirft · / Befehle · @ Dateien%%*')
  :format(to:gsub('%%', '%%%%'))

vim.api.nvim_set_hl(0, 'ColliemsgCommand', { link = 'Function', default = true })
vim.api.nvim_set_hl(0, 'ColliemsgFile', { link = 'Underlined', default = true })
vim.fn.matchadd('ColliemsgCommand', [[\v(^\s*)@<=/[[:alnum:]_:.-]+]])
vim.fn.matchadd('ColliemsgFile', [[\v(^|\s)@<=\@\S+]])

-- ---- Slash-Befehle ---------------------------------------------------------

local BUILTIN = {
  { 'clear', 'Verlauf leeren und neu beginnen' },
  { 'compact', 'Verlauf zusammenfassen, optional mit Anweisung' },
  { 'context', 'Kontextverbrauch anzeigen' },
  { 'cost', 'Kosten der Session anzeigen' },
  { 'usage', 'Nutzung und Limits anzeigen' },
  { 'model', 'Modell wechseln' },
  { 'review', 'Pull Request prüfen' },
  { 'security-review', 'Sicherheitsprüfung der offenen Änderungen' },
  { 'pr-comments', 'Kommentare eines Pull Requests holen' },
  { 'init', 'CLAUDE.md für das Projekt anlegen' },
  { 'memory', 'Memory-Dateien bearbeiten' },
  { 'resume', 'Frühere Session fortsetzen' },
  { 'rewind', 'Zu einem früheren Stand zurück' },
  { 'export', 'Verlauf exportieren' },
  { 'add-dir', 'Weiteres Arbeitsverzeichnis hinzufügen' },
  { 'agents', 'Subagents verwalten' },
  { 'mcp', 'MCP-Server verwalten' },
  { 'permissions', 'Berechtigungen ansehen und ändern' },
  { 'hooks', 'Hooks verwalten' },
  { 'config', 'Einstellungen öffnen' },
  { 'status', 'Status von Claude Code' },
  { 'todos', 'Aktuelle Todo-Liste' },
  { 'help', 'Hilfe' },
}

local function read_lines(path, max)
  local ok, lines = pcall(vim.fn.readfile, path, '', max)
  return ok and lines or {}
end

local function strip_quotes(value)
  return (value:gsub('^["\']', ''):gsub('["\']$', ''))
end

--- Felder aus dem YAML-Frontmatter, nur einfache und gefaltete Werte.
local function frontmatter(path)
  local lines = read_lines(path, 60)
  local meta, key = {}, nil
  if lines[1] ~= '---' then
    return meta
  end
  for i = 2, #lines do
    local line = lines[i]
    if line == '---' then
      break
    end
    local k, v = line:match('^([%w_-]+):%s*(.-)%s*$')
    if k then
      key = k
      if v == '>' or v == '|' or v == '>-' or v == '|-' then
        v = ''
      end
      meta[k] = strip_quotes(v)
    elseif key and line:match('^%s+%S') then
      meta[key] = vim.trim(meta[key] .. ' ' .. vim.trim(line))
    end
  end
  return meta
end

local commands_cache

local function commands()
  if commands_cache then
    return commands_cache
  end
  local items, seen = {}, {}
  local function add(name, description, kind)
    if not name or name == '' or seen[name] then
      return
    end
    seen[name] = true
    description = description or ''
    local menu = description
    if vim.fn.strchars(menu) > 60 then
      menu = vim.fn.strcharpart(menu, 0, 59) .. '…'
    end
    table.insert(items, {
      word = name,
      abbr = '/' .. name,
      menu = menu,
      info = description,
      kind = kind,
      icase = 1,
    })
  end
  local function add_skills(dir, prefix)
    for _, path in ipairs(vim.fn.glob(dir .. '/*/SKILL.md', false, true)) do
      local meta = frontmatter(path)
      local name = (meta.name and meta.name ~= '') and meta.name or vim.fn.fnamemodify(path, ':h:t')
      add(prefix .. name, meta.description, 'skill')
    end
  end
  local function add_commands(dir, prefix)
    for _, path in ipairs(vim.fn.globpath(dir, '**/*.md', false, true)) do
      local rel = path:sub(#dir + 2):gsub('%.md$', ''):gsub('/', ':')
      add(prefix .. rel, frontmatter(path).description, 'befehl')
    end
  end

  -- Projekt zuerst, dann Nutzer, dann Plugins, zuletzt die eingebauten.
  local roots = { cwd }
  local git_root = vim.fs.root(cwd, '.git')
  if git_root and git_root ~= cwd then
    table.insert(roots, git_root)
  end
  for _, root in ipairs(roots) do
    add_skills(root .. '/.claude/skills', '')
    add_commands(root .. '/.claude/commands', '')
  end
  add_skills(claude_home .. '/skills', '')
  add_commands(claude_home .. '/commands', '')

  local ok, installed = pcall(function()
    return vim.json.decode(table.concat(read_lines(claude_home .. '/plugins/installed_plugins.json'), '\n'))
  end)
  if ok and type(installed) == 'table' and type(installed.plugins) == 'table' then
    for id, installs in pairs(installed.plugins) do
      local plugin = id:match('^([^@]+)') or id
      for _, install in ipairs(type(installs) == 'table' and installs or {}) do
        if type(install) == 'table' and type(install.installPath) == 'string' then
          add_skills(install.installPath .. '/skills', plugin .. ':')
          add_commands(install.installPath .. '/commands', plugin .. ':')
        end
      end
    end
  end

  for _, builtin in ipairs(BUILTIN) do
    add(builtin[1], builtin[2], 'claude')
  end
  commands_cache = items
  return items
end

-- ---- Dateien ---------------------------------------------------------------

local files_loaded = false

--- Dateien und Ordner relativ zum Arbeitsverzeichnis des Chats, als
--- `b:collie_files`, damit `matchfuzzy` sie nicht bei jedem Tastendruck aus
--- Lua übernehmen muss.
local function load_files()
  if files_loaded then
    return
  end
  files_loaded = true
  local paths = vim.fn.systemlist({ 'git', '-C', cwd, 'ls-files', '--cached', '--others', '--exclude-standard' })
  if vim.v.shell_error ~= 0 then
    paths = {}
    local skip = function(name)
      return not name:match('^%.') and name ~= 'node_modules' and name ~= 'target'
    end
    for name, kind in vim.fs.dir(cwd, { depth = 5, skip = skip }) do
      table.insert(paths, kind == 'directory' and (name .. '/') or name)
      if #paths >= MAX_FILES then
        break
      end
    end
  else
    local dirs, seen = {}, {}
    for _, path in ipairs(paths) do
      local dir = path:match('^(.*)/[^/]*$')
      while dir and not seen[dir] do
        seen[dir] = true
        table.insert(dirs, dir .. '/')
        dir = dir:match('^(.*)/[^/]*$')
      end
    end
    vim.list_extend(paths, dirs)
  end
  if #paths > MAX_FILES then
    paths = vim.list_slice(paths, 1, MAX_FILES)
  end
  table.sort(paths, function(a, b)
    return #a < #b or (#a == #b and a < b)
  end)
  vim.b[buf].collie_files = paths
end

local function file_items(query)
  load_files()
  local matches
  if query == '' then
    matches = vim.list_slice(vim.b[buf].collie_files or {}, 1, MAX_ITEMS)
  else
    matches = vim.api.nvim_eval(
      ('matchfuzzy(b:collie_files, %s, {"limit": %d})'):format(vim.fn.string(query), MAX_ITEMS)
    )
  end
  local items = {}
  for _, path in ipairs(matches) do
    table.insert(items, { word = path, kind = path:match('/$') and 'ordner' or 'datei' })
  end
  return items
end

local function command_items(query)
  local all = commands()
  if query == '' then
    return vim.list_slice(all, 1, MAX_ITEMS)
  end
  return vim.fn.matchfuzzy(all, query, { key = 'word', limit = MAX_ITEMS })
end

-- ---- Auslöser --------------------------------------------------------------

--- Art, Startspalte (1-basiert) und Suchtext des Tokens vor dem Cursor.
local function token()
  local line = vim.api.nvim_get_current_line()
  local col = vim.api.nvim_win_get_cursor(0)[2]
  local before = line:sub(1, col)
  local slash = before:match('^%s*/([%w_:%.%-]*)$')
  if slash then
    return 'slash', col - #slash + 1, slash
  end
  local at = before:match('^@(%S*)$') or before:match('%s@(%S*)$')
  if at then
    return 'file', col - #at + 1, at
  end
end

local skip_next = false

local function trigger()
  if vim.fn.mode():sub(1, 1) ~= 'i' then
    return
  end
  if skip_next then
    skip_next = false
    return
  end
  local kind, start, query = token()
  if not kind then
    return
  end
  local items = kind == 'slash' and command_items(query) or file_items(query)
  vim.fn.complete(start, items)
end

local group = vim.api.nvim_create_augroup('CollieMessage', { clear = true })
vim.api.nvim_create_autocmd({ 'TextChangedI', 'TextChangedP' }, {
  group = group,
  buffer = buf,
  callback = trigger,
})
-- Nach einer gewählten Datei nicht sofort wieder aufklappen; nach einem
-- Ordner schon, damit es darin weitergeht.
vim.api.nvim_create_autocmd('CompleteDone', {
  group = group,
  buffer = buf,
  callback = function()
    local word = (vim.v.completed_item or {}).word
    if word and word ~= '' and not word:match('/$') then
      skip_next = true
    end
  end,
})

local function map(modes, lhs, rhs, opts)
  vim.keymap.set(modes, lhs, rhs, vim.tbl_extend('force', { buffer = buf }, opts or {}))
end
map('i', '<CR>', function()
  if vim.fn.pumvisible() == 1 and vim.fn.complete_info({ 'selected' }).selected ~= -1 then
    return '<C-y>'
  end
  return '<CR>'
end, { expr = true })
map('i', '<Tab>', function()
  return vim.fn.pumvisible() == 1 and '<C-n>' or '<Tab>'
end, { expr = true })
map('i', '<S-Tab>', function()
  return vim.fn.pumvisible() == 1 and '<C-p>' or '<S-Tab>'
end, { expr = true })
map({ 'n', 'i' }, '<C-s>', '<Cmd>wq<CR>')

vim.cmd('startinsert')
