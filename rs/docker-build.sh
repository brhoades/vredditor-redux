#!/usr/bin/env bash
cp -R ../proto .
trap "rm -rf ./proto" EXIT ERR
docker build -t vredditor-api:latest .
