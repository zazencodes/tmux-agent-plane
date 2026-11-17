local M = {}

local function error_message(message)
  vim.notify(message, vim.log.levels.ERROR, { title = "tmux-agent-plane" })
end

local function run(args)
  local command = vim.list_extend({ "tmux-agent-plane" }, args)
  local ok, result = pcall(function()
    return vim.system(command, { text = true }):wait()
  end)
  if not ok then
    error_message(tostring(result))
    return
  end
  if result.code ~= 0 then
    error_message(table.concat(command, " ") .. ": " .. vim.trim(result.stderr .. result.stdout))
    return
  end
  return result.stdout
end

-- The workspace this Neovim's tmux session belongs to, as its sidebar and picker show it.
local function workspace_here()
  local output = run({ "workspace", "list", "--json" })
  if not output then
    return
  end
  local ok, rows = pcall(vim.json.decode, output)
  if not ok or type(rows) ~= "table" or not vim.islist(rows) or not output:match("^%s*%[") then
    error_message("tmux-agent-plane workspace list --json: expected an array of workspaces")
    return
  end
  for _, row in ipairs(rows) do
    if type(row) == "table" and row.here == true and type(row.name) == "string" then
      return row.name
    end
  end
  error_message("tmux-agent-plane: cannot tell which workspace this pane is in; run Neovim inside tmux")
end

local function agents(workspace, history)
  local command = history and "recent" or "list"
  local output = run({ command, "--json" })
  if not output then
    return
  end
  local ok, rows = pcall(vim.json.decode, output)
  if not ok or type(rows) ~= "table" or not vim.islist(rows) or not output:match("^%s*%[") then
    error_message("tmux-agent-plane " .. command .. " --json: expected an array of agents")
    return
  end
  local fields = history and { "name", "title", "session", "workspace", "kind" }
    or { "name", "title", "session", "workspace", "pane", "state", "location" }
  for i, row in ipairs(rows) do
    if history then
      if type(row) ~= "table" or type(row.id) ~= "number" or type(row.agent) ~= "table" or type(row.closed_at) ~= "number"
        or type(row.project_color) ~= "string" or not row.project_color:match("^#%x%x%x%x%x%x$") then
        error_message("tmux-agent-plane recent --json: invalid history entry")
        return
      end
      row.agent.project_color = row.project_color
      row.agent.history_id = row.id
      row.agent.closed_at = row.closed_at
      rows[i] = row.agent
      row = row.agent
    end
    for _, field in ipairs(fields) do
      if type(row) ~= "table" or type(row[field]) ~= "string" then
        error_message("tmux-agent-plane " .. command .. " --json: invalid agent field " .. field)
        return
      end
    end
    if not history and not row.pane:match("^%%%d+$") then
      error_message("tmux-agent-plane list --json: invalid agent pane " .. row.pane)
      return
    end
  end
  return vim.tbl_filter(function(row)
    return row.workspace == workspace
  end, rows)
end

local function pick(opts, history)
  opts = opts or {}
  local workspace = workspace_here()
  if not workspace then
    return
  end
  local rows = agents(workspace, history)
  if not rows then
    return
  end

  local actions = require("telescope.actions")
  local action_state = require("telescope.actions.state")
  local project = history and opts.project or nil
  local function finder()
    local visible = project and vim.tbl_filter(function(a) return a.session == project end, rows) or rows
    return require("telescope.finders").new_table({
      results = visible,
      entry_maker = function(agent)
        if history then
          local title = agent.title ~= "" and agent.title or agent.name
          local group = "AgentPlaneProject" .. agent.project_color:sub(2)
          vim.api.nvim_set_hl(0, group, { fg = agent.project_color })
          return {
            value = agent,
            ordinal = title .. " " .. agent.name .. " " .. agent.session .. " " .. agent.kind,
            display = function()
              local prefix = "▏ " .. agent.session
              local when = os.date("%b %d %H:%M", math.floor(agent.closed_at / 1000))
              return prefix .. "  " .. title .. "  ·  " .. when, { { { 0, #prefix }, group } }
            end,
          }
        end
        local detail = type(agent.detail) == "string" and agent.detail or ""
        local description = agent.state == "blocked" and type(agent.detail) == "string" and detail or agent.title
        return {
          value = agent,
          ordinal = agent.name .. " " .. agent.session .. " " .. agent.title .. " " .. detail,
          display = agent.name .. (agent.orchestrator and " [orch]" or "") .. "  " .. description .. "  [" .. agent.state .. " · " .. agent.location .. "]",
        }
      end,
    })
  end
  local picker = require("telescope.pickers").new(opts, {
    prompt_title = (history and "Recently closed · " or "Agents · ") .. workspace,
    prompt_prefix = project and (project .. " › ") or opts.prompt_prefix,
    previewer = false,
    finder = finder(),
    sorter = require("telescope.sorters").get_fzy_sorter(opts),
    attach_mappings = function(prompt_bufnr, map)
      for _, mode in ipairs({ "i", "n" }) do
        map(mode, "<Down>", actions.move_selection_next)
        map(mode, "<Up>", actions.move_selection_previous)
      end
      if history then
        map("n", "p", function()
          local entry = action_state.get_selected_entry()
          if project then project = nil
          elseif entry then project = entry.value.session
          else return end
          local current = action_state.get_current_picker(prompt_bufnr)
          current:refresh(finder(), {
            new_prefix = project and (project .. " › ") or opts.prompt_prefix or require("telescope.config").values.prompt_prefix,
            reset_prompt = false,
          })
        end)
        map("n", "x", function()
          local entry = action_state.get_selected_entry()
          if not entry then return end
          local a = entry.value
          if vim.fn.confirm("Remove " .. a.name .. " from history?", "&Yes\n&No", 2) == 1
            and run({ "forget", tostring(a.history_id) }) then
            rows = vim.tbl_filter(function(row) return row.history_id ~= a.history_id end, rows)
            action_state.get_current_picker(prompt_bufnr):refresh(finder(), { reset_prompt = false })
          end
        end)
      end
      map("n", "h", function()
        actions.close(prompt_bufnr)
        pick(opts, not history)
      end)
      actions.select_default:replace(function()
        local entry = action_state.get_selected_entry()
        actions.close(prompt_bufnr)
        if entry and history then
          run({ "revive", tostring(entry.value.history_id), "--focus" })
        elseif entry then
          run({ "focus", entry.value.pane })
        end
      end)
      return true
    end,
  })
  picker:find()
  return picker
end

function M.agents(opts) return pick(opts, false) end
function M.history(opts) return pick(opts, true) end

return M
