/* Accessors for LibRaw fields the C API has no setter/getter for (output params, colour
 * data). Compiled against the installed LibRaw headers by build.rs, so struct layouts
 * always match the linked library version. */
#include <string.h>
#include <libraw/libraw.h>

/* Linear camera-RGB develop source: no white balance, no colour conversion, no gamma,
 * no auto-brightness, 16-bit, no rotation, highlights clipped at the white level. */
void sieve_lr_set_linear(libraw_data_t *lr, int half_size)
{
  libraw_output_params_t *p = &lr->params;
  p->half_size = half_size;
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
}
