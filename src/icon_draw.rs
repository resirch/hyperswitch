use image::{imageops::FilterType, Rgba, RgbaImage};
use windows::Win32::Foundation::HANDLE;
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

/// Scale an icon handle to the requested square size. Many game window icons
/// are 16–32 px; DrawIconEx will not upscale them and they appear tiny in the
/// corner unless we copy/scale the handle first.
unsafe fn scaled_icon_handle(icon: HICON, size: i32) -> (HICON, bool) {
    match CopyImage(
        HANDLE(icon.0 as *mut _),
        IMAGE_ICON,
        size,
        size,
        LR_DEFAULTCOLOR,
    ) {
        Ok(scaled) if !scaled.is_invalid() => (HICON(scaled.0 as *mut _), true),
        _ => (icon, false),
    }
}

unsafe fn rasterize_icon(icon: HICON, size: i32) -> Option<RgbaImage> {
    let (draw_icon, owned) = scaled_icon_handle(icon, size);

    let result = rasterize_icon_handle(draw_icon, size);

    if owned {
        let _ = DestroyIcon(draw_icon);
    }

    result.map(|img| normalize_icon_content(&img, size as u32))
}

unsafe fn rasterize_icon_handle(icon: HICON, size: i32) -> Option<RgbaImage> {
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

/// Crop to visible pixels and scale the artwork to fill the square slot.
fn normalize_icon_content(img: &RgbaImage, size: u32) -> RgbaImage {
    let (min_x, min_y, max_x, max_y) = content_bounds(img);
    if max_x <= min_x || max_y <= min_y {
        return image::imageops::resize(img, size, size, FilterType::Lanczos3);
    }

    let w = max_x - min_x + 1;
    let h = max_y - min_y + 1;

    // Already fills the raster — just downscale to the display size later.
    if w as f32 >= img.width() as f32 * 0.85 && h as f32 >= img.height() as f32 * 0.85 {
        return img.clone();
    }

    let cropped = image::imageops::crop_imm(img, min_x, min_y, w, h).to_image();
    let side = w.max(h).max(1);
    let mut square = RgbaImage::new(side, side);
    let ox = ((side - w) / 2) as i64;
    let oy = ((side - h) / 2) as i64;
    image::imageops::overlay(&mut square, &cropped, ox, oy);

    let scaled = image::imageops::resize(&square, size, size, FilterType::Lanczos3);
    let mut canvas = RgbaImage::new(size, size);
    image::imageops::overlay(&mut canvas, &scaled, 0, 0);
    canvas
}

fn content_bounds(img: &RgbaImage) -> (u32, u32, u32, u32) {
    let mut min_x = u32::MAX;
    let mut min_y = u32::MAX;
    let mut max_x = 0u32;
    let mut max_y = 0u32;

    for y in 0..img.height() {
        for x in 0..img.width() {
            let Rgba([r, g, b, a]) = *img.get_pixel(x, y);
            if a > 16 || (r | g | b) > 16 {
                min_x = min_x.min(x);
                min_y = min_y.min(y);
                max_x = max_x.max(x);
                max_y = max_y.max(y);
            }
        }
    }

    if max_x < min_x {
        (0, 0, 0, 0)
    } else {
        (min_x, min_y, max_x, max_y)
    }
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
