#!/usr/bin/env python3
from __future__ import print_function

import os
import sys

for name in sys.argv[1:]:
    print(os.environ.get(name))
