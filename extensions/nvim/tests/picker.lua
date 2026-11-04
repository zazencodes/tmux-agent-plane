-- Run by test_picker.py inside its private tmux server, using real Telescope.
local root = assert(vim.env.PICKER_TEST_ROOT)
local function write(path, value)
  vim.fn.writefile(vim.split(value, "\n", { plain = true }), root .. "/" .. path)
end
local function wait_for(predicate, message)
  assert(vim.wait(3000, predicate, 10), message)
end
local function keys(input)
  vim.api.nvim_feedkeys(vim.api.nvim_replace_termcodes(input, true, false, true), "xt", false)
end

local function test()
  vim.opt.rtp:append(assert(vim.env.PICKER_EXTENSION))
  vim.opt.rtp:append(assert(vim.env.PICKER_TELESCOPE))
  vim.opt.rtp:append(assert(vim.env.PICKER_PLENARY))
  local telescope = require("telescope")
  telescope.setup({ defaults = { sorting_strategy = "ascending", initial_mode = "normal" } })
  telescope.load_extension("tmux_agent_plane")
  local notifications = {}
  vim.notify = function(message, level)
    notifications[#notifications + 1] = { message = message, level = level }
  end
  local actions = require("telescope.actions")
  local function open(count)
    local picker = telescope.extensions.tmux_agent_plane.agents()
    assert(picker, "picker did not open")
    wait_for(function() return picker.manager and picker.manager:num_results() == count end, "result count")
    return picker
  end
  local function close(picker)
    actions.close(picker.prompt_bufnr)
  end
  local function mode(value)
    write("mode", value)
    -- The harness refreshes the private CLI index for each scenario.
    wait_for(function() return vim.fn.readfile(root .. "/ready")[1] == value end, "index scenario " .. value)
  end

  mode("normal")
  if vim.env.PICKER_DOTFILES then
    local dotfiles = vim.env.PICKER_DOTFILES .. "/nvim/lua/plugins/"
    -- Capture lazy's declarations without loading plugins or installing anything.
    local plugins
    package.loaded.lazy = { setup = function(spec) plugins = spec end }
    local stat = vim.uv.fs_stat
    vim.uv.fs_stat = function(path)
      if path == vim.fn.stdpath("data") .. "/lazy/lazy.nvim" then
        return { type = "directory" }
      end
      return stat(path)
    end
    local ok, message = pcall(dofile, dotfiles .. "lazy.lua")
    vim.uv.fs_stat = stat
    assert(ok, message)
    local local_plugin
    for _, plugin in ipairs(plugins) do
      if plugin.name == "tmux-agent-plane.nvim" then local_plugin = plugin end
    end
    assert(local_plugin and local_plugin.dir == vim.fn.expand("~/pro/tmux-agent-plane/extensions/nvim"))
    assert(local_plugin.dependencies[1] == "nvim-telescope/telescope.nvim")
    vim.opt.rtp:append(vim.env.PICKER_UI_SELECT)
    vim.g.mapleader = " "
    dofile(dotfiles .. "tele.lua")
    dofile(dotfiles .. "keymaps.lua")
    assert(vim.fn.maparg(" fs", "n"):lower() == ":telescope find_files<cr>", "fs changed")
    assert(vim.fn.maparg(" fz", "n"):find("live_grep", 1, true), "fz changed")
    local binding = vim.fn.maparg(" fa", "n", false, true)
    assert(type(binding.callback) == "function", "Space f a missing")
    binding.callback()
    local configured_picker = require("telescope.actions.state").get_current_picker(vim.api.nvim_get_current_buf())
    wait_for(function() return configured_picker.manager and configured_picker.manager:num_results() == 2 end, "Space f a results")
    close(configured_picker)
    -- Keep subsequent keyboard checks deterministic in the headless harness.
    telescope.setup({ defaults = { sorting_strategy = "ascending", initial_mode = "normal" } })
    write("config-checks", "PASS: local lazy plugin declaration; actual tele.lua extension load and Space f a callback; existing fs/fz mappings preserved")
  end
  local picker = open(2)
  local first = picker:get_selection()
  assert(first.value.name == "cedar-review")
  keys("<Down>")
  assert(picker:get_selection().value.name == "quartz-build", "normal Down")
  keys("<Up>")
  assert(picker:get_selection() == first, "normal Up")
  keys("i<Down><Esc>")
  assert(picker:get_selection().value.name == "quartz-build", "insert Down")
  keys("i<Up><Esc>")
  assert(picker:get_selection() == first, "insert Up")
  assert(vim.fn.filereadable(root .. "/focus.calls") == 0, "browsing focused an agent")
  close(picker)

  for _, query in ipairs({ "QZBD", "SCPR", "APCR", "QZ APCR", "quartz Ship" }) do
    picker = open(2)
    keys("i" .. query .. "<Esc>")
    wait_for(function()
      return picker.manager:num_results() == 1 and picker:get_selection()
        and picker:get_selection().value.name == "quartz-build"
    end, "fuzzy query: " .. query)
    local entry = picker:get_selection()
    assert(entry.ordinal:match("^quartz%-build "), "name must lead ordinal")
    assert(entry.ordinal:find("Ship compiler", 1, true), "title missing")
    assert(entry.ordinal:find("Approve cache refresh", 1, true), "detail missing")
    assert(entry.display:find("Approve cache refresh", 1, true), "blocked description missing")
    close(picker)
  end
  picker = open(2)
  keys("iimpossiblexyz<Esc>")
  wait_for(function() return picker.manager:num_results() == 0 end, "no search matches")
  keys("<CR>")
  assert(vim.fn.filereadable(root .. "/focus.calls") == 0, "empty selection focused an agent")

  mode("empty")
  picker = open(0)
  keys("<Down><Up><CR>")
  assert(#notifications == 0, "empty list reported an error")

  for _, scenario in ipairs({ "stale", "malformed", "object", "bad-pane", "failure" }) do
    mode(scenario)
    local previous = #notifications
    assert(telescope.extensions.tmux_agent_plane.agents() == nil)
    assert(#notifications == previous + 1 and notifications[#notifications].level == vim.log.levels.ERROR,
      "missing error notification: " .. scenario)
    if scenario == "stale" then
      assert(notifications[#notifications].message:find("daemon is not running", 1, true), "stale CLI error hidden")
    end
  end

  mode("focus-failure")
  picker = open(2)
  keys("<Down><CR>")
  assert(notifications[#notifications].message:find("injected focus failure", 1, true), "focus error hidden")

  -- A fresh list may rename the same agent. Selection must still use its pane.
  mode("normal")
  picker = open(2)
  keys("<Down>")
  local selected = picker:get_selection()
  selected.value.name = "renamed-after-selection"
  write("selected-pane", selected.value.pane)
  write("checks", "PASS: real Telescope arrows in both modes; case-insensitive name/title/detail and spanning fuzzy queries; empty/no-match Enter; stale/malformed/list/focus errors; browsing never focuses")
  write("before-focus", "ready")
  wait_for(function() return vim.fn.filereadable(root .. "/allow-focus") == 1 end, "two-client focus setup")
  keys("i<CR>")
  write("done", "ok")
end

local ok, message = xpcall(test, debug.traceback)
if not ok then
  write("failure", message)
  vim.cmd("cquit 1")
else
  vim.cmd("qa!")
end
