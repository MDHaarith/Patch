//! The per-window back buffer the software renderer draws into, and the
//! rectangle bookkeeping needed to copy only what changed to the screen.

use slint::platform::software_renderer::{PhysicalRegion, PremultipliedRgbaColor};

// `PremultipliedRgbaColor` is `#[repr(C)] {red, green, blue, alpha}` of u8,
// byte-for-byte what Android's RGBA_8888/RGBX_8888 buffers expect.
const _: () = assert!(std::mem::size_of::<PremultipliedRgbaColor>() == 4);
const _: () = assert!(std::mem::align_of::<PremultipliedRgbaColor>() == 1);

/// A pixel rectangle; `w == 0 || h == 0` means empty.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Rect {
    pub x: u32,
    pub y: u32,
    pub w: u32,
    pub h: u32,
}

impl Rect {
    pub fn new(x: u32, y: u32, w: u32, h: u32) -> Rect {
        Rect { x, y, w, h }
    }

    pub fn full(w: u32, h: u32) -> Rect {
        Rect { x: 0, y: 0, w, h }
    }

    pub fn is_empty(&self) -> bool {
        self.w == 0 || self.h == 0
    }

    pub fn right(&self) -> u32 {
        self.x + self.w
    }

    pub fn bottom(&self) -> u32 {
        self.y + self.h
    }

    pub fn union(self, other: Rect) -> Rect {
        if self.is_empty() {
            return other;
        }
        if other.is_empty() {
            return self;
        }
        let x = self.x.min(other.x);
        let y = self.y.min(other.y);
        Rect {
            x,
            y,
            w: self.right().max(other.right()) - x,
            h: self.bottom().max(other.bottom()) - y,
        }
    }

    /// The part of `self` inside a `w`×`h` area at the origin.
    pub fn clamp(self, w: u32, h: u32) -> Rect {
        let x = self.x.min(w);
        let y = self.y.min(h);
        Rect {
            x,
            y,
            w: self.right().min(w) - x,
            h: self.bottom().min(h) - y,
        }
    }

    /// The bounding box of what the renderer reports as repainted.
    pub fn bounds_of(region: &PhysicalRegion) -> Rect {
        let o = region.bounding_box_origin();
        let s = region.bounding_box_size();
        Rect {
            x: o.x.max(0) as u32,
            y: o.y.max(0) as u32,
            w: s.width,
            h: s.height,
        }
    }
}

/// A persistent RGBA frame. With `RepaintBufferType::ReusedBuffer` the
/// renderer only repaints dirty regions, so the contents must survive between
/// frames — including while the Android surface itself is gone.
#[derive(Default)]
pub struct BackBuffer {
    width: u32,
    height: u32,
    pixels: Vec<PremultipliedRgbaColor>,
}

impl BackBuffer {
    pub fn width(&self) -> u32 {
        self.width
    }

    pub fn height(&self) -> u32 {
        self.height
    }

    pub fn is_empty(&self) -> bool {
        self.width == 0 || self.height == 0
    }

    /// Reallocates (cleared to transparent) if the size changed. Returns
    /// whether it did, in which case the caller must repaint everything.
    pub fn resize(&mut self, width: u32, height: u32) -> bool {
        if width == self.width && height == self.height {
            return false;
        }
        self.width = width;
        self.height = height;
        self.pixels = vec![PremultipliedRgbaColor::default(); width as usize * height as usize];
        // Give the memory of a large old frame back right away (rotation).
        self.pixels.shrink_to_fit();
        true
    }

    pub fn pixels_mut(&mut self) -> &mut [PremultipliedRgbaColor] {
        &mut self.pixels
    }

    /// The frame as tightly packed RGBA bytes.
    pub fn bytes(&self) -> &[u8] {
        // SAFETY: the pixel type is four u8 with alignment 1 (asserted above),
        // so any pixel slice is a valid byte slice four times as long.
        unsafe {
            std::slice::from_raw_parts(self.pixels.as_ptr().cast::<u8>(), self.pixels.len() * 4)
        }
    }

    /// Copies `rect` of the frame into a destination of `dst_w`×`dst_h`
    /// pixels whose rows are `dst_stride` pixels apart. Parts of `rect` that
    /// lie inside the destination but outside the frame (the surface is
    /// briefly larger than the frame while a resize is in flight) are cleared
    /// so no stale memory reaches the screen.
    ///
    /// # Safety
    /// `dst` must be valid for writes of `dst_stride * dst_h * 4` bytes.
    pub unsafe fn copy_to(
        &self,
        rect: Rect,
        dst: *mut u8,
        dst_w: u32,
        dst_h: u32,
        dst_stride: u32,
    ) {
        let rect = rect.clamp(dst_w, dst_h);
        let src = self.bytes();
        for y in rect.y..rect.bottom() {
            let row = dst.add((y as usize * dst_stride as usize + rect.x as usize) * 4);
            let row_len = rect.w as usize * 4;
            if y >= self.height {
                std::ptr::write_bytes(row, 0, row_len);
                continue;
            }
            let copy_w = rect.right().min(self.width).saturating_sub(rect.x) as usize;
            if copy_w > 0 {
                let from = (y as usize * self.width as usize + rect.x as usize) * 4;
                std::ptr::copy_nonoverlapping(src.as_ptr().add(from), row, copy_w * 4);
            }
            if copy_w * 4 < row_len {
                std::ptr::write_bytes(row.add(copy_w * 4), 0, row_len - copy_w * 4);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rect_union_and_clamp() {
        let a = Rect::new(10, 10, 5, 5);
        let b = Rect::new(0, 12, 2, 10);
        assert_eq!(a.union(b), Rect::new(0, 10, 15, 12));
        assert_eq!(a.union(Rect::default()), a);
        assert_eq!(
            Rect::new(8, 8, 10, 10).clamp(12, 20),
            Rect::new(8, 8, 4, 10)
        );
        assert!(Rect::new(30, 0, 5, 5).clamp(12, 20).is_empty());
    }

    #[test]
    fn copy_honours_stride_and_clears_overhang() {
        let mut frame = BackBuffer::default();
        assert!(frame.resize(2, 2));
        assert!(!frame.resize(2, 2));
        for (i, p) in frame.pixels_mut().iter_mut().enumerate() {
            *p = PremultipliedRgbaColor {
                red: i as u8 + 1,
                green: 0,
                blue: 0,
                alpha: 255,
            };
        }
        // Destination: 3x3 pixels, stride 4, pre-filled with 0xAA.
        let mut dst = vec![0xAAu8; 4 * 3 * 4];
        unsafe { frame.copy_to(Rect::full(3, 3), dst.as_mut_ptr(), 3, 3, 4) };
        let px = |x: usize, y: usize| &dst[(y * 4 + x) * 4..(y * 4 + x) * 4 + 4];
        assert_eq!(px(0, 0), [1, 0, 0, 255]);
        assert_eq!(px(1, 0), [2, 0, 0, 255]);
        assert_eq!(px(0, 1), [3, 0, 0, 255]);
        assert_eq!(px(1, 1), [4, 0, 0, 255]);
        assert_eq!(px(2, 0), [0, 0, 0, 0]);
        assert_eq!(px(0, 2), [0, 0, 0, 0]);
        // The padding column beyond the destination width is untouched.
        assert_eq!(px(3, 0), [0xAA; 4]);
    }
}
