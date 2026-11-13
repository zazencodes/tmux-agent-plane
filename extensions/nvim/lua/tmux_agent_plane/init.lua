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

local function agents(workspace)
  local output = run({ "list", "--json" })
  if not output then
    return
  end
  local ok, rows = pcall(vim.json.decode, output)
  if not ok or type(rows) ~= "table" or not vim.islist(rows) or not output:match("^%s*%[") then
    error_message("tmux-agent-plane list --json: expected an array of agents")
    return
  end
  for _, row in ipairs(rows) do
    for _, field in ipairs({ "name", "title", "session", "workspace", "pane", "state", "location" }) do
      if type(row) ~= "table" or type(row[field]) ~= "string" then
        error_message("tmux-agent-plane list --json: invalid agent field " .. field)
        return
      end
    end
    if not row.pane:match("^%%%d+$") then
      error_message("tmux-agent-plane list --json: invalid agent pane " .. row.pane)
      return
    end
  end
  return vim.tbl_filter(function(row)
    return row.workspace == workspace
  end, rows)
end

function M.agents(opts)
  opts = opts or {}
  local workspace = workspace_here()
  if not workspace then
    return
  end
  local rows = agents(workspace)
  if not rows then
    return
  end

  local actions = require("telescope.actions")
  local action_state = require("telescope.actions.state")
  local picker = require("telescope.pickers").new(opts, {
    prompt_title = "Agents · " .. workspace,
    previewer = false,
    finder = require("telescope.finders").new_table({
      results = rows,
      entry_maker = function(agent)
        local detail = type(agent.detail) == "string" and agent.detail or ""
        local description = agent.state == "blocked" and type(agent.detail) == "string" and detail or agent.title
        return {
          value = agent,
          ordinal = agent.name .. " " .. agent.session .. " " .. agent.title .. " " .. detail,
          display = agent.name .. "  " .. description .. "  [" .. agent.state .. " · " .. agent.location .. "]",
        }
      end,
    }),
    sorter = require("telescope.sorters").get_fzy_sorter(opts),
    attach_mappings = function(prompt_bufnr, map)
      for _, mode in ipairs({ "i", "n" }) do
        map(mode, "<Down>", actions.move_selection_next)
        map(mode, "<Up>", actions.move_selection_previous)
      end
      actions.select_default:replace(function()
        local entry = action_state.get_selected_entry()
        actions.close(prompt_bufnr)
        if entry then
          run({ "focus", entry.value.pane })
        end
      end)
      return true
    end,
  })
  picker:find()
  return picker
end

return M
