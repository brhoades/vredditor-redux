#!/usr/bin/env bash
cp -R ../proto .
trap "rm -rf ./proto" EXIT ERR
docker build -t vredditorapilocal:latest .
