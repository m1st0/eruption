id = '5dc59fa6-e989-55cb-a0da-e87d28713339'
name = 'Heartbeat: System Monitor'
description = 'A system monitor effect'
active_scripts = [
	'heartbeat.lua',
	'afterhue.lua',
#   'dim-zone.lua',
 	'macros.lua',
#	'stats.lua',
]

# dim a specific zone, e.g. if the mouse LEDs are too bright
[[config."Dim Zone"]]
type = 'int'
name = 'zone_start'
value = 144
default = 144

[[config."Dim Zone"]]
type = 'int'
name = 'zone_end'
value = 180
default = 180

[[config."Dim Zone"]]
type = 'float'
name = 'opacity'
value = 0.95
default = 0.95
