use image::{imageops::FilterType, Rgba, RgbaImage};
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::UI::WindowsAndMessaging::*;

/// Source resolution used when rasterizing an HICON before Lanczos downscale.
/// Higher values produce smoother anti-aliased edges at the display size.
const RASTER_SIZE: i32 = 256;

/// Rasterize an HICON to RGBA at `RASTER_SIZE`, downscale with Lanczos, and
/// alpha-composite it onto a premultiplied-ARGB destination buffer.
pub unsafe fn draw_icon_smooth(
    dest: &mut [u32],
    dest_w: i32,
    dest_h: i32,
    x: i32,
    y: i32,
    icon: HICON,
    size: i32,
) {
    if icon.0.is_null() || size <= 0 {
        return;
    }

    let Some(source) = rasterize_icon(icon, RASTER_SIZE) else {
        return;
    };
    let resized = image::imageops::resize(&source, size as u32, size as u32, FilterType::Lanczos3);
    blend_rgba_over_premult(dest, dest_w, dest_h, x, y, &resized);
}

unsafe fn rasterize_icon(icon: HICON, size: i32) -> Option<RgbaImage> {
    let screen = GetDC(None);
    if screen.is_invalid() {
        return None;
    }
    let mem_dc = CreateCompatibleDC(Some(screen));
    if mem_dc.is_invalid() {
        ReleaseDC(None, screen);
        return None;
    }

    let bi = BITMAPINFO {
        bmiHeader: BITMAPINFOHEADER {
            biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: size,
            biHeight: -size,
            biPlanes: 1,
            biBitCount: 32,
            biCompression: BI_RGB.0,
            ..Default::default()
        },
        ..Default::default()
    };
    let mut bits: *mut core::ffi::c_void = std::ptr::null_mut();
    let dib = CreateDIBSection(Some(mem_dc), &bi, DIB_RGB_COLORS, &mut bits, None, 0);
    let dib = match dib {
        Ok(b) if !bits.is_null() => b,
        _ => {
            let _ = DeleteDC(mem_dc);
            ReleaseDC(None, screen);
            return None;
        }
    };

    let old = SelectObject(mem_dc, dib.into());
    std::ptr::write_bytes(bits as *mut u8, 0, (size as usize) * (size as usize) * 4);
    let _ = DrawIconEx(mem_dc, 0, 0, icon, size, size, 0, None, DI_NORMAL);
    let _ = GdiFlush();

    let pixel_count = (size as usize) * (size as usize);
    let raw = std::slice::from_raw_parts(bits as *const u32, pixel_count);
    let mut rgba = Vec::with_capacity(pixel_count * 4);
    for &p in raw {
        // GDI 32-bit DIB pixels are BGRA in memory (little-endian u32).
        rgba.push(((p >> 16) & 0xff) as u8); // R
        rgba.push(((p >> 8) & 0xff) as u8);  // G
        rgba.push((p & 0xff) as u8);         // B
        rgba.push(((p >> 24) & 0xff) as u8); // A
    }

    // GDI often leaves the alpha byte at 0 even for visible icon pixels.
    if !rgba.chunks_exact(4).any(|px| px[3] > 0) {
        for px in rgba.chunks_exact_mut(4) {
            if px[0] | px[1] | px[2] != 0 {
                px[3] = 255;
            }
        }
    }

    SelectObject(mem_dc, old);
    let _ = DeleteObject(dib.into());
    let _ = DeleteDC(mem_dc);
    ReleaseDC(None, screen);

    RgbaImage::from_raw(size as u32, size as u32, rgba)
}

/// Porter-Duff "over" composite of an unpremultiplied RGBA icon onto a
/// premultiplied-ARGB destination.
fn blend_rgba_over_premult(
    dest: &mut [u32],
    dest_w: i32,
    dest_h: i32,
    x: i32,
    y: i32,
    icon: &RgbaImage,
) {
    let iw = icon.width() as i32;
    let ih = icon.height() as i32;

    for dy in 0..ih {
        let dest_y = y + dy;
        if dest_y < 0 || dest_y >= dest_h {
            continue;
        }
        for dx in 0..iw {
            let dest_x = x + dx;
            if dest_x < 0 || dest_x >= dest_w {
                continue;
            }

            let Rgba([sr, sg, sb, sa]) = *icon.get_pixel(dx as u32, dy as u32);
            if sa == 0 {
                continue;
            }

            let idx = (dest_y * dest_w + dest_x) as usize;
            let dp = dest[idx];
            let da = ((dp >> 24) & 0xff) as f32 / 255.0;
            let dr = ((dp >> 16) & 0xff) as f32;
            let dg = ((dp >> 8) & 0xff) as f32;
            let db = (dp & 0xff) as f32;

            let sa_f = sa as f32 / 255.0;
            let inv_sa = 1.0 - sa_f;

            // Unpremultiply destination for the blend, then re-premultiply.
            let (dr_u, dg_u, db_u) = if da > 0.0 {
                (dr / da, dg / da, db / da)
            } else {
                (0.0, 0.0, 0.0)
            };

            let sr_f = sr as f32;
            let sg_f = sg as f32;
            let sb_f = sb as f32;

            let out_a = sa_f + da * inv_sa;
            if out_a <= 0.0 {
                continue;
            }

            let out_r = (sr_f * sa_f + dr_u * da * inv_sa) / out_a;
            let out_g = (sg_f * sa_f + dg_u * da * inv_sa) / out_a;
            let out_b = (sb_f * sa_f + db_u * da * inv_sa) / out_a;

            let a = (out_a * 255.0).round().clamp(0.0, 255.0) as u32;
            let r = (out_r * out_a).round().clamp(0.0, 255.0) as u32;
            let g = (out_g * out_a).round().clamp(0.0, 255.0) as u32;
            let b = (out_b * out_a).round().clamp(0.0, 255.0) as u32;

            dest[idx] = (a << 24) | (r << 16) | (g << 8) | b;
        }
    }
}
