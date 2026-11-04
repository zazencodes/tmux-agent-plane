return require("telescope").register_extension({
  exports = {
    agents = require("tmux_agent_plane").agents,
  },
})
