#!/usr/bin/env python3
"""Host-only original AV1 CBS unit retention + IVF/software parity proof.

No VA calls or hardware decoders. IVF adds container framing only, without
synthesizing sequence/frame headers. This is not VAAPI transport qualification.
"""
import argparse,hashlib,json,struct,subprocess
from pathlib import Path
from fractions import Fraction

def hashes(path):return [line.split(',')[-1].strip() for line in path.read_text().splitlines() if line and not line.startswith('#')]
def digest(path):return hashlib.sha256(path.read_bytes()).hexdigest()
def main():
 p=argparse.ArgumentParser(description=__doc__);p.add_argument('probe',type=Path);p.add_argument('output',type=Path);p.add_argument('samples',nargs='+',type=Path);a=p.parse_args();a.output.mkdir(parents=True,exist_ok=False);results=[]
 for i,sample in enumerate(a.samples):
  label=f'{i}-{sample.stem}';folder=a.output/label;folder.mkdir();raw=folder/'original-units.obu';index=folder/'units.jsonl'
  with (folder/'producer.log').open('x') as log:subprocess.run([str(a.probe.resolve()),str(sample),str(raw),str(index)],stdout=log,stderr=subprocess.STDOUT,check=True,timeout=30)
  metadata=json.loads(subprocess.run(['/usr/bin/ffprobe','-v','error','-count_frames','-select_streams','v:0','-show_entries','stream=codec_name,width,height,nb_read_frames,time_base,pix_fmt','-of','json',str(sample)],check=True,capture_output=True,text=True,timeout=30).stdout)['streams'][0]
  if metadata['codec_name']!='av1' or not (0<int(metadata['width'])<65536 and 0<int(metadata['height'])<65536):raise ValueError('fixture_identity_shape')
  formats={'yuv420p':'nv12','yuv420p10le':'yuv420p10le','yuv420p12le':'yuv420p12le'}
  if metadata.get('pix_fmt') not in formats:raise ValueError('unsupported_reference_pixel_format')
  pixel_format=formats[metadata['pix_fmt']]
  unit_records=[json.loads(line) for line in index.read_text().splitlines()];body=raw.read_bytes();position=0;packets={};pts={};extra=b''
  for unit in unit_records:
   size=unit['bytes'];data=body[position:position+size]
   if size<=0 or len(data)!=size:raise ValueError('retained_unit_bounds')
   tile_size=unit['tile_bytes'];tile_offset=unit['tile_offset']
   if tile_size<0 or tile_offset<0 or tile_offset+tile_size>size:raise ValueError('original_tile_span_bounds')
   if (unit['type']==4 or (unit['type']==6 and unit['show_existing']==0)) and tile_size==0:raise ValueError('missing_original_tile_span')
   position+=size;number=unit['packet']
   if number==-1:extra+=data
   else:
    packets.setdefault(number,bytearray()).extend(data);pts.setdefault(number,unit['pts'])
  if position!=len(body) or sorted(packets)!=list(range(len(packets))) or not packets:raise ValueError('unit_membership_or_packet_order')
  packets[0]=extra+packets[0];timebase=Fraction(metadata['time_base']);wrapped=folder/'retained.ivf'
  with wrapped.open('xb') as out:
   out.write(struct.pack('<4sHH4sHHIIII',b'DKIF',0,32,b'AV01',int(metadata['width']),int(metadata['height']),timebase.denominator,timebase.numerator,len(packets),0))
   for number,data in packets.items():
    if pts[number]<0:raise ValueError('unavailable_packet_timestamp')
    out.write(struct.pack('<IQ',len(data),pts[number]));out.write(data)
  for name,source in [('reference',sample),('retained',wrapped)]:
   with (folder/(name+'.log')).open('x') as log:subprocess.run(['/usr/bin/ffmpeg','-nostdin','-hide_banner','-v','error','-c:v','libdav1d','-threads:v','1','-i',str(source),'-map','0:v:0','-an','-pix_fmt',pixel_format,'-threads:v','1','-fps_mode','passthrough','-f','framemd5',str(folder/(name+'.md5'))],stdout=log,stderr=subprocess.STDOUT,check=True,timeout=30)
  ref=hashes(folder/'reference.md5');actual=hashes(folder/'retained.md5');valid=ref==actual and len(ref)==int(metadata['nb_read_frames'])
  result={'sample':str(sample),'status':'pass' if valid else 'fail','displayed_frames':len(actual),'source_pixel_format':metadata['pix_fmt'],'comparison_pixel_format':pixel_format,'source_sha256':digest(sample),'retained_units_sha256':digest(raw),'unit_count':len(unit_records),'original_tile_groups':sum(u['tile_bytes']>0 for u in unit_records),'hidden_headers':sum(u['show_frame']==0 for u in unit_records),'show_existing_headers':sum(u['show_existing']==1 for u in unit_records),'scope':'host original sequence/frame/tile byte retention and software pixel/order roundtrip; not hardware or VA integration'};results.append(result);(a.output/'result.json').write_text(json.dumps(results,indent=2)+'\n');print(json.dumps(result),flush=True)
  if not valid:return 1
 return 0
if __name__=='__main__':raise SystemExit(main())
