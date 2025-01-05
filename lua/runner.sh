#!/usr/bin/env bash

# TODO: Do this in lua with require() instead

cargo b --quiet

for test in tests/*.lua; do
  echo "Running $test"
  LC_ALL=en_US.UTF-8 ../target/debug/ftlman lua-run "$test"
done

