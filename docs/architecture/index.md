---
layout: default
title: Architecture
nav_order: 2
has_children: true
---

# Architecture

This section covers the internal design of Allux — how the components are organized, how they communicate, and why each design decision was made.

The most important component to understand first is the **Context Manager**, as it drives the vast majority of Allux's effectiveness with local models.

For multi-step structured execution, see the **[Orchestra Engine](orchestra.md)** — it covers the state machine, planner, validator, diagnoser, worker, store, and event stream.
