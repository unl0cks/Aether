//! Shared admission and translation for drawing several objects' commands in one pass.
//!
//! Two graduated flocks use this: filtered cache entries (`render_content_atlased_group`)
//! and complex-blend children (`resolve_deferred_blend_children`). Both used to cost one
//! render pass per member on a renderer priced by passes, and both group the same way --
//! translate each member's commands by whole pixels into a slot of one shared surface and
//! draw them together. Whole-pixel translation moves rasterised pixels exactly, and
//! disjoint slots keep members from compositing over each other.

use ruffle_render::commands::{Command, CommandList};
use swf::Twips;

/// Whether these commands can share one render pass with other members' contents.
///
/// The bar is that they stay in one `Chunk::Draw`. A complex or shader blend composites
/// against "the target", which in a shared surface would be the neighbours; even a trivial
/// `Blend` command spawns a sub-target sized to the whole surface unless it is a sole
/// carried draw, and in a shared surface the whole surface is everyone's. Alpha masks and
/// Stage3D have machinery of their own, and a perspective projection does not commute with
/// the slot translation.
///
/// Masks may group, but only if they BALANCE. A member with its own target could leave a
/// mask standing at the end with nothing but its own target to spoil; on a shared surface
/// the stencil outlives the member and would clip whoever draws next.
pub fn commands_are_groupable(commands: &CommandList) -> bool {
    let mut mask_depth: i32 = 0;
    let drawable = commands.commands.iter().all(|command| match command {
        Command::RenderBitmap { transform, .. } | Command::RenderShape { transform, .. } => {
            transform.perspective_projection.is_none()
        }
        Command::DrawRect { .. } | Command::DrawLine { .. } | Command::DrawLineRect { .. } => true,
        Command::PushMask => {
            mask_depth += 1;
            true
        }
        Command::PopMask => {
            mask_depth -= 1;
            mask_depth >= 0
        }
        Command::ActivateMask | Command::DeactivateMask => true,
        Command::RenderStage3D { .. } | Command::RenderAlphaMask { .. } | Command::Blend(..) => {
            false
        }
    });
    drawable && mask_depth == 0
}

/// The same commands, shifted so they draw into a slot instead of at their recorded place.
///
/// Only reached for command lists [`commands_are_groupable`] admitted, so the variants
/// without a translatable transform cannot appear; they are reproduced unchanged anyway
/// rather than trusted to stay unreachable.
pub fn commands_translated(commands: &CommandList, dx: Twips, dy: Twips) -> Vec<Command> {
    let translate = |matrix: &ruffle_render::matrix::Matrix| {
        let mut moved = *matrix;
        moved.tx += dx;
        moved.ty += dy;
        moved
    };
    commands
        .commands
        .iter()
        .map(|command| match command {
            Command::RenderBitmap {
                bitmap,
                transform,
                smoothing,
                pixel_snapping,
                source_size,
            } => Command::RenderBitmap {
                bitmap: bitmap.clone(),
                transform: ruffle_render::transform::Transform {
                    matrix: translate(&transform.matrix),
                    ..transform.clone()
                },
                smoothing: *smoothing,
                pixel_snapping: *pixel_snapping,
                source_size: *source_size,
            },
            Command::RenderShape { shape, transform } => Command::RenderShape {
                shape: shape.clone(),
                transform: ruffle_render::transform::Transform {
                    matrix: translate(&transform.matrix),
                    ..transform.clone()
                },
            },
            Command::DrawRect { color, matrix } => Command::DrawRect {
                color: *color,
                matrix: translate(matrix),
            },
            Command::DrawLine { color, matrix } => Command::DrawLine {
                color: *color,
                matrix: translate(matrix),
            },
            Command::DrawLineRect { color, matrix } => Command::DrawLineRect {
                color: *color,
                matrix: translate(matrix),
            },
            other => other.clone(),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use ruffle_render::matrix::Matrix;
    use ruffle_render::transform::Transform;

    fn rect_at(x: f64, y: f64) -> Command {
        Command::DrawRect {
            color: swf::Color::WHITE,
            matrix: Matrix::translate(Twips::from_pixels(x), Twips::from_pixels(y)),
        }
    }

    #[test]
    fn translation_moves_every_draw_and_keeps_the_count() {
        let mut list = CommandList::new();
        list.commands = vec![rect_at(10.0, 20.0), Command::PushMask, rect_at(0.0, 0.0)];
        let moved = commands_translated(&list, Twips::from_pixels(5.0), Twips::from_pixels(7.0));
        assert_eq!(moved.len(), 3);
        match &moved[0] {
            Command::DrawRect { matrix, .. } => {
                assert_eq!(matrix.tx, Twips::from_pixels(15.0));
                assert_eq!(matrix.ty, Twips::from_pixels(27.0));
            }
            other => panic!("expected a rect, got {other:?}"),
        }
        assert!(matches!(moved[1], Command::PushMask));
    }

    /// An unbalanced mask outlives its member on a shared surface and would clip whoever
    /// draws next, which a member with a target of its own could never do.
    #[test]
    fn an_unbalanced_mask_is_refused() {
        let mut dangling = CommandList::new();
        dangling.commands = vec![Command::PushMask, Command::ActivateMask, rect_at(0.0, 0.0)];
        assert!(!commands_are_groupable(&dangling));

        let mut extra_pop = CommandList::new();
        extra_pop.commands = vec![Command::PopMask];
        assert!(!commands_are_groupable(&extra_pop));
    }

    #[test]
    fn blends_and_perspective_are_refused_but_masks_are_not() {
        let mut plain = CommandList::new();
        plain.commands = vec![Command::PushMask, rect_at(0.0, 0.0), Command::PopMask];
        assert!(commands_are_groupable(&plain));

        let mut blended = CommandList::new();
        blended.commands = vec![Command::Blend(
            CommandList::new(),
            ruffle_render::commands::RenderBlendMode::Builtin(swf::BlendMode::Multiply),
            None,
        )];
        assert!(!commands_are_groupable(&blended));

        let mut perspective = CommandList::new();
        perspective.commands = vec![Command::RenderShape {
            shape: ruffle_render::backend::ShapeHandle(std::sync::Arc::new(TestShape)),
            transform: Transform {
                matrix: Matrix::IDENTITY,
                color_transform: Default::default(),
                perspective_projection: Some(Default::default()),
            },
        }];
        assert!(!commands_are_groupable(&perspective));
    }

    #[derive(Debug)]
    struct TestShape;
    impl ruffle_render::backend::ShapeHandleImpl for TestShape {}
}
