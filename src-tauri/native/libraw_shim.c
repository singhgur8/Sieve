/* Accessors for LibRaw fields the C API has no setter/getter for (output params, colour
 * data). Compiled against the installed LibRaw headers by build.rs, so struct layouts
 * always match the linked library version. */
#include <string.h>
#include <libraw/libraw.h>

/* Linear camera-RGB develop source: no white balance, no colour conversion, no gamma,
 * no auto-brightness, 16-bit, no rotation, highlights clipped at the white level.
 * Full size (half_size = 0) demosaics with user_qual = 3: AHD for Bayer; LibRaw runs its
 * 3-pass Markesteijn interpolation for X-Trans at that quality. */
void sieve_lr_set_linear(libraw_data_t *lr, int half_size)
{
  libraw_output_params_t *p = &lr->params;
  p->half_size = half_size;
  p->user_qual = half_size ? -1 : 3;
  p->use_camera_wb = 0;
  p->use_auto_wb = 0;
  p->user_mul[0] = p->user_mul[1] = p->user_mul[2] = p->user_mul[3] = 1.0f;
  p->output_color = 0;
  p->gamm[0] = 1.0;
  p->gamm[1] = 1.0;
  p->no_auto_bright = 1;
  p->bright = 1.0f;
  p->output_bps = 16;
  p->user_flip = 0;
  p->highlight = 0;
  p->adjust_maximum_thr = 0.0f;
  p->user_black = -1;
  p->user_sat = -1;
}

typedef struct {
  float cam_mul[4];
  float pre_mul[4];
  float rgb_cam[3][4];
  float cam_xyz[4][3];
  unsigned black;
  unsigned maximum;
  int width;
  int height;
  int flip;
  int colors;
  unsigned filters;
  char make[64];
  char model[64];
  float fuji_expo_shift;
  float dng_baseline_exposure;
  unsigned linear_max;
  /* Camera WB presets (R, G, B, G2): Daylight (EXIF light source 1) and D65 (21). */
  int wb_daylight[4];
  int wb_d65[4];
  /* Default crop ("raw inset", = Adobe's DefaultCrop) in raw coordinates and the image
   * origin in raw coordinates. */
  unsigned inset[4]; /* left, top, width, height */
  unsigned margin[2]; /* left, top */
} sieve_lr_color_t;

/* Colour data after open_file (before processing rewrites pre_mul). */
void sieve_lr_get_color(libraw_data_t *lr, sieve_lr_color_t *out)
{
  memcpy(out->cam_mul, lr->color.cam_mul, sizeof out->cam_mul);
  memcpy(out->pre_mul, lr->color.pre_mul, sizeof out->pre_mul);
  memcpy(out->rgb_cam, lr->color.rgb_cam, sizeof out->rgb_cam);
  memcpy(out->cam_xyz, lr->color.cam_xyz, sizeof out->cam_xyz);
  out->black = lr->color.black;
  out->maximum = lr->color.maximum;
  out->width = lr->sizes.width;
  out->height = lr->sizes.height;
  out->flip = lr->sizes.flip;
  out->colors = lr->idata.colors;
  out->filters = lr->idata.filters;
  /* LibRaw's normalized names ("Sony", "ILCE-7M4"; "Canon", "EOS M6 Mark II"). */
  memcpy(out->make, lr->idata.normalized_make, sizeof out->make);
  memcpy(out->model, lr->idata.normalized_model, sizeof out->model);
  out->make[sizeof out->make - 1] = 0;
  out->model[sizeof out->model - 1] = 0;
  out->fuji_expo_shift = lr->makernotes.fuji.ExpoMidPointShift;
  out->dng_baseline_exposure = lr->color.dng_levels.baseline_exposure;
  out->linear_max = lr->color.linear_max[0];
  memcpy(out->wb_daylight, lr->color.WB_Coeffs[LIBRAW_WBI_Daylight], sizeof out->wb_daylight);
  memcpy(out->wb_d65, lr->color.WB_Coeffs[LIBRAW_WBI_D65], sizeof out->wb_d65);
  out->inset[0] = lr->sizes.raw_inset_crops[0].cleft;
  out->inset[1] = lr->sizes.raw_inset_crops[0].ctop;
  out->inset[2] = lr->sizes.raw_inset_crops[0].cwidth;
  out->inset[3] = lr->sizes.raw_inset_crops[0].cheight;
  out->margin[0] = lr->sizes.left_margin;
  out->margin[1] = lr->sizes.top_margin;
}

/* Clips/scales at `white` (absolute raw units) instead of LibRaw's maximum. Call after
 * unpack (black levels known). LibRaw applies user_sat to the black-subtracted range when
 * the black level lives in cblack (CR3), so the per-channel black is taken off here. */
void sieve_lr_set_white(libraw_data_t *lr, unsigned white)
{
  unsigned black = lr->color.black;
  unsigned cb = lr->color.cblack[0];
  for (int i = 1; i < 4; i++)
    if (lr->color.cblack[i] < cb)
      cb = lr->color.cblack[i];
  black += cb;
  if (white > black + 1024)
    lr->params.user_sat = (int)(white - black);
}

/* Copies the processed image (after dcraw_process) as interleaved RGB16 into `out`
 * (capacity `cap` values), skipping dcraw_make_mem_image's extra full-size buffer. The
 * output curve make_mem_image would apply is the identity for our settings (gamma 1/1,
 * no auto-bright, white 0x10000). Returns 0 on success, -1 if the layout is not a plain
 * 3-colour image (the caller then falls back to make_mem_image), -2 if `cap` is short.
 * Writes the image size to `w`/`h` (call with out = NULL to query). */
int sieve_lr_copy_rgb16(libraw_data_t *lr, unsigned short *out, size_t cap, int *w, int *h)
{
  if (!lr->image || lr->idata.colors != 3 || lr->rawdata.ioparams.fuji_width)
    return -1;
  int iw = lr->sizes.iwidth, ih = lr->sizes.iheight;
  *w = iw;
  *h = ih;
  size_t n = (size_t)iw * (size_t)ih;
  if (!out)
    return 0;
  if (cap < n * 3)
    return -2;
  for (size_t i = 0; i < n; i++)
  {
    out[i * 3 + 0] = lr->image[i][0];
    out[i * 3 + 1] = lr->image[i][1];
    out[i * 3 + 2] = lr->image[i][2];
  }
  return 0;
}
