id = '5dc62fa6-e965-45cb-a0da-e87d29713102'
name = 'Gaming: Generic'
description = 'The generic gaming profile'
active_scripts = [
    'solid.lua',
#   'impact.lua',
#   'water.lua',
    'gaming.lua',
#   'dim-zone.lua',
    'macros.lua',
#   'stats.lua',
]

[[config."Solid Color"]]
type = 'color'
name = 'color_background'
value = 0xff1f1f1f
default = 0xff1f1f1f

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
