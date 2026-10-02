#!/usr/bin/env python3
"""Host-only actual-function AV1 empty-capture discard model. No decoder opens.

Injects capture completions into actual ff_v4l2_context_dequeue_frame and the
actual FFmpeg receive-frame core. Packet submission and firmware are unmodeled.
"""
import argparse,hashlib,subprocess,tempfile
from pathlib import Path

def extract(text,name):
 start=text.index(name+'(');start=text.rfind('\n',0,start)+1;brace=text.index('{',start);depth=1;i=brace+1
 while depth:
  depth+=(text[i]=='{')-(text[i]=='}');i+=1
 return text[start:i]

STUBS=r'''
#include <assert.h>
#include <errno.h>
#include <stdio.h>
#include <string.h>
#include <stdint.h>
#define AVERROR(n) (-(n))
#define AVERROR_EOF -1000
#define AV_FRAME_FLAG_DISCARD 4
#define AV_CODEC_ID_AV1 1
#define AVMEDIA_TYPE_AUDIO 2
#define FF_CODEC_CB_TYPE_RECEIVE_FRAME 1
#define V4L2_BUF_FLAG_ERROR 8
#define V4L2_BUF_FLAG_LAST 16
#define V4L2_TYPE_IS_OUTPUT(t) ((t)==2)
#define V4L2_TYPE_IS_MULTIPLANAR(t) ((t)==1)
#define av_assert0(n) assert(n)
#define emms_c() ((void)0)
typedef struct AVFrame { void *buf[1]; int flags,pict_type; } AVFrame;
typedef struct DecodeContext { int initial_pict_type,intra_only_flag; } DecodeContext;
typedef struct AVCodecInternal { int draining_done; DecodeContext dc; } AVCodecInternal;
typedef struct AVCodec { int type; } AVCodec;
typedef struct AVCodecContext { int codec_id; AVCodecInternal *internal; AVCodec *codec; } AVCodecContext;
typedef struct FFCodec { int cb_type; struct { int (*receive_frame)(AVCodecContext*,AVFrame*); } cb; } FFCodec;
typedef struct Plane { unsigned bytesused; } Plane;
typedef struct Buffer { unsigned flags,bytesused; struct { Plane *planes; } m; } Buffer;
typedef struct V4L2Buffer { Buffer buf; Plane planes[1]; } V4L2Buffer;
typedef struct V4L2m2mContext { AVCodecContext *avctx; } V4L2m2mContext;
typedef struct V4L2Context { int done,type; V4L2m2mContext parent; } V4L2Context;
static V4L2Context ctx;
static V4L2Buffer buffer;
static FFCodec codec;
static int empties,produced,requeued,mapped,baseline,queue_error;
static const FFCodec *ffcodec(AVCodec *c) { (void)c;return &codec; }
static DecodeContext *decode_ctx(AVCodecInternal *c) { return &c->dc; }
static void av_frame_unref(AVFrame *f) { memset(f,0,sizeof(*f)); }
static int discard_samples(AVCodecContext *c,AVFrame*f,int64_t*n) { (void)c;(void)f;(void)n;return 0; }
static int decode_simple_receive_frame(AVCodecContext*c,AVFrame*f) { (void)c;(void)f;return AVERROR(EINVAL); }
static V4L2m2mContext *ctx_to_m2mctx(V4L2Context *c) { return &c->parent; }
static V4L2Buffer *v4l2_dequeue_v4l2buf(V4L2Context*c,int timeout) {
 (void)timeout;
 if (empties) {
  empties--;if(baseline)return NULL;
  memset(&buffer,0,sizeof(buffer));buffer.buf.flags=V4L2_BUF_FLAG_ERROR;
  buffer.buf.m.planes=buffer.planes;return &buffer;
 }
 if(produced){c->done=1;return NULL;}
 produced=1;memset(&buffer,0,sizeof(buffer));buffer.buf.m.planes=buffer.planes;
 buffer.planes[0].bytesused=128;return &buffer;
}
static int ff_v4l2_buffer_enqueue(V4L2Buffer*b) { (void)b;requeued++;return queue_error?AVERROR(EIO):0; }
static int ff_v4l2_buffer_buf_to_avframe(AVFrame*f,V4L2Buffer*b) { (void)b;mapped++;f->buf[0]=(void*)1;return 0; }
'''
MAIN=r'''
static int receive(AVCodecContext*c,AVFrame*f) { (void)c;return ff_v4l2_context_dequeue_frame(&ctx,f,-1); }
static void setup(AVCodecContext*c,int n) {
 ctx=(V4L2Context){.type=1,.parent={.avctx=c}};empties=n;produced=requeued=mapped=baseline=queue_error=0;
 codec=(FFCodec){.cb_type=FF_CODEC_CB_TYPE_RECEIVE_FRAME,.cb={.receive_frame=receive}};
 memset(c->internal,0,sizeof(*c->internal));
}
int main(void) {
 AVCodecInternal internal={0};AVCodec avcodec={0};AVCodecContext c={.codec_id=AV_CODEC_ID_AV1,.internal=&internal,.codec=&avcodec};AVFrame frame={0};
 setup(&c,1);baseline=1;
 assert(ff_decode_receive_frame_internal(&c,&frame)==AVERROR(EAGAIN));assert(!mapped&&!requeued);
 puts("baseline_return_null=EAGAIN_before_frame_reproduced");
 for(int n=0;n<=10000;n=n? n*10:1) {
  setup(&c,n);av_frame_unref(&frame);
  assert(ff_decode_receive_frame_internal(&c,&frame)==0);
  assert(frame.buf[0]&&mapped==1&&requeued==n&&!(frame.flags&AV_FRAME_FLAG_DISCARD));
  av_frame_unref(&frame);assert(ff_decode_receive_frame_internal(&c,&frame)==AVERROR_EOF);
  assert(internal.draining_done);
 }
 setup(&c,1);queue_error=1;av_frame_unref(&frame);
 assert(ff_decode_receive_frame_internal(&c,&frame)==AVERROR(EIO));assert(!mapped&&requeued==1);
 c.codec_id=2;setup(&c,1);av_frame_unref(&frame);
 assert(ff_decode_receive_frame_internal(&c,&frame)==0);assert(mapped==1&&!requeued);
 puts("candidate_discard_core=pass sequences=0,1,10,100,1000,10000 requeue_error=propagated other_codec=unchanged");
 return 0;
}
'''
def main():
 p=argparse.ArgumentParser(description=__doc__);p.add_argument('source',type=Path);a=p.parse_args()
 files=[a.source/'libavcodec/v4l2_context.c',a.source/'libavcodec/decode.c']
 chunks=[extract(f.read_text(),n) for f,n in zip(files,['ff_v4l2_context_dequeue_frame','ff_decode_receive_frame_internal'])]
 with tempfile.TemporaryDirectory(prefix='ffmpeg-av1-discard-host.') as tmp:
  src=Path(tmp)/'model.c';exe=Path(tmp)/'model';src.write_text(STUBS+'\n'+'\n'.join(chunks)+MAIN)
  subprocess.run(['cc','-std=c11','-O2','-Wall','-Wextra','-Werror',str(src),'-o',str(exe)],check=True)
  subprocess.run([str(exe)],check=True,timeout=5)
 for f in files:print('source_sha256='+hashlib.sha256(f.read_bytes()).hexdigest()+' file='+str(f))
if __name__=='__main__':main()
