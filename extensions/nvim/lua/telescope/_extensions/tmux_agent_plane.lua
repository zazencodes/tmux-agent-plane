return require("telescope").register_extension({
  exports = {
    history = require("tmux_agent_plane").history,
    agents = require("tmux_agent_plane").agents,
  },
})
