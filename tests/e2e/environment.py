# SPDX-License-Identifier: MPL-2.0
"""Validate the environment recorded by a native regression journey."""

ENVIRONMENT_FIELDS = frozenset(('os_family', 'os_build', 'architecture', 'hardware', 'mode',
                                'theme', 'dpi', 'renderer', 'assistive_technology', 'build_mode'))


def require(condition, message):
    if not condition:
        raise ValueError(message)


def validate(value):
    require(isinstance(value, dict) and value.keys() == ENVIRONMENT_FIELDS,
            'Environment must contain exactly the test identity fields')
    require(all(isinstance(item, str) and item.strip() == item and 0 < len(item) <= 512
                and not any(ord(c) < 32 for c in item) for item in value.values()),
            'Environment values must be bounded nonempty strings')
    choices = {'os_family': {'windows', 'linux', 'macos'}, 'architecture': {'x64', 'arm64'},
               'mode': {'keyboard', 'pointer', 'screen_reader', 'headless'},
               'theme': {'dark', 'light', 'high_contrast', 'not_applicable'},
               'dpi': {'100', '125', '150', '200', '250', '300', 'not_applicable'},
               'renderer': {'software', 'hardware', 'not_applicable'},
               'build_mode': {'preview', 'configured', 'fixture'}}
    for key, allowed in choices.items():
        require(value[key] in allowed, 'Unsupported environment ' + key)
    require(value['mode'] != 'screen_reader' or value['assistive_technology'] != 'none',
            'Screen-reader environment must identify the assistive technology/version')
    return dict(value)
