---
status: accepted
---

# Targeting is a ray cast, with assist only for Enemies

A Gesture fires a Tongue by casting a ray along the tapped vector. Whatever terrain that ray hits is the Anchor. The aim is never bent, adjusted, or snapped.

This is worth recording because the obvious instinct on a touch device is the opposite. An earlier version of this design carried a 30 degree tolerance cone that selected the best Anchor near the tapped direction, and it was removed deliberately: a future reader looking at thumb-sized touch targets will want to add aim assist back.

## Why assist is unnecessary for terrain

The cone existed to solve one problem: if only scattered points in a level are anchorable, a thumb cannot reliably hit a small one, especially while covering it.

That problem does not exist here. Every surface of the level is anchorable, floors and lower walls included, so a ray along the tapped direction always lands on something valid. With nothing to disambiguate, assist can only do harm: it would bend the aim away from the surface the player actually pointed at, which on a universally grappable level is always a worse answer than the literal one.

Universal anchorability is itself load-bearing rather than a simplification. Anchoring downward to the floor is how a player builds descent speed, and the canonical scenario for a high `toughness` ground Enemy is to hop up, anchor to the floor, and strike from above. A level approach will not break it.

## Enemies keep a narrow assist

Enemies are small and moving, so one narrow **Enemy Assist Arc** survives, scoped to them alone. An Enemy inside the arc is preferred over the static terrain directly behind it. This expresses a target preference and nothing else: it never alters the ray's direction, so the hero still attaches along exactly the vector that was tapped.

## Movable Anchors attach at the centre of mass

A Tongue attaching to an Enemy or any other Movable Anchor attaches at its exact centre of mass, never at the surface point the ray struck and never at an offset attachment node. This keeps the spring force collinear with the line between the two bodies, which is what makes the mass-ratio pull in ADR-0003 and the tension calculation in ADR-0011 well defined. A surface attachment point would introduce a torque the simulation has no way to represent, since bodies here have no angular dynamics.

## There is no whiff

Releasing retracts the Tongue instantly with no delay. Combined with unlimited range and universally anchorable terrain, a fired Tongue always connects eventually, so there is no miss case to cost the player anything and no recovery animation to design around.

## Consequences

Range is unlimited and distance costs time rather than reach: a far surface simply takes longer before the attach lands. Tongue launch speed is per-hero and is the primary traversal differentiator, with low-tier heroes carrying real delay in repositioning and high-tier heroes able to make sharp corrections against distant surfaces.

Because terrain is the Anchor set, level data needs no separate anchor list; `static_terrain` serves both roles. See ADR-0010.
