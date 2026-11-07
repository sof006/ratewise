-- Written by Ratewise. It regenerates this file on every `ratewise set --save`,
-- so edits here are lost. `ratewise reset` removes it.
bluez_monitor = bluez_monitor or {}
bluez_monitor.rules = bluez_monitor.rules or {}

-- Bluetooth Headset
table.insert(bluez_monitor.rules, {
  matches = {
    {
      { "device.name", "equals", "bluez_card.00_00_00_00_00_03" },
    },
  },
  apply_properties = {
    ["bluez5.a2dp.ldac.quality"] = "mq",
  },
})
