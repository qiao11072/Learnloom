"""Render original Learnloom book/ribbon geometry into Windows multi-size icons.
The editable vector source is src/assets/learnloom.svg. No third-party art or fonts.
"""
from pathlib import Path
from PIL import Image, ImageDraw
ROOT=Path(__file__).resolve().parent.parent
S=4
im=Image.new('RGBA',(256*S,256*S))
d=ImageDraw.Draw(im)
scale=lambda p:tuple(int(v*S) for v in p)
def q(a,b,c):
 return [((1-t)**2*a[0]+2*(1-t)*t*b[0]+t*t*c[0],(1-t)**2*a[1]+2*(1-t)*t*b[1]+t*t*c[1]) for t in [i/32 for i in range(33)]]
def line(points,color,width):
 pts=[scale(p) for p in points];d.line(pts,fill=color,width=width*S,joint='curve')
 for x,y in [pts[0],pts[-1]]:
  r=width*S/2;d.ellipse((x-r,y-r,x+r,y+r),fill=color)
d.rounded_rectangle(scale((8,8,248,248)),radius=56*S,fill='#245D49')
left=q((48,66),(87,56),(122,80))+[(122,192)]+q((122,192),(87,169),(48,179))
right=q((134,80),(169,56),(208,66))+[(208,179)]+q((208,179),(169,169),(134,192))
d.polygon([scale(p) for p in left],fill='#F5F3DD');d.polygon([scale(p) for p in right],fill='#BFD8B1')
for y in (91,115,139):line(q((68,y),(86,y-1),(102,y+10)),'#8BAA83',6)
ribbon=[(128,68),(128,137)]+q((128,137),(128,151),(143,151))+[(181,151)]
line(ribbon,'#245D49',23);line(ribbon,'#E6BD73',12)
out=ROOT/'src-tauri/icons';out.mkdir(exist_ok=True)
im.resize((512,512),Image.Resampling.LANCZOS).save(out/'icon.png')
im.save(out/'icon.ico',sizes=[(n,n) for n in (16,20,24,32,40,48,64,128,256)])
preview=Image.new('RGB',(720,320),'#F5F7F3')
for x,n in [(30,256),(330,128),(500,64),(610,32)]:
 icon=im.resize((n,n),Image.Resampling.LANCZOS);preview.paste(icon,(x,(320-n)//2),icon)
preview.save(ROOT/'docs/release/artifacts/rc4-icon-preview.png')
print('Original vector-style book and woven L icon rendered.')
