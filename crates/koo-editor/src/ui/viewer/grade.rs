//! Grades on decoded RGBA for preview: [`koo_core::grade`], the same code
//! the scopes use and the export bakes into its LUT.

use koo_core::model::Grade;

/// Grade RGBA pixels in place.
pub fn apply(rgba: &mut [u8], g: &Grade) {
    koo_core::grade::apply(rgba, g);
}
