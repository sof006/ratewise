-- Written by Ratewise. It regenerates this file on every `ratewise set --save`,
-- so edits here are lost. `ratewise reset` removes it.
alsa_monitor = alsa_monitor or {}
alsa_monitor.rules = alsa_monitor.rules or {}

-- Built-in Audio Analog Stereo
table.insert(alsa_monitor.rules, {
  matches = {
    {
      { "node.name", "equals", "alsa_output.pci-0000_00_1f.3.analog-stereo" },
    },
  },
  apply_properties = {
    ["audio.format"] = "S16LE",
    ["api.alsa.headroom"] = 1024,
    ["session.suspend-timeout-seconds"] = 0,
  },
})
