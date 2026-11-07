-- Written by Ratewise. It regenerates this file on every `ratewise set --save`,
-- so edits here are lost. `ratewise reset` removes it.
bluetooth_policy = bluetooth_policy or {}
bluetooth_policy.policy = bluetooth_policy.policy or {}

-- Keep Bluetooth headsets in their mode when a program uses the microphone.
bluetooth_policy.policy["media-role.use-headset-profile"] = false
