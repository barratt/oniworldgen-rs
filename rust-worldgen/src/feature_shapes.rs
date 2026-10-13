//! Ordered raster point lists used by foreground terrain features. Duplicates
//! in circles and lines are intentional: placement can draw fresh RNG per entry.
use crate::{geom::V2,sampler::SeededRandom};
use crate::fxhash::FxHashSet as HashSet;
pub type Point=(i32,i32);

pub fn line(mut start:V2,end:V2)->Vec<Point> {
    let dx=(end.x-start.x).abs();let dy=(end.y-start.y).abs();
    let sx=if start.x<end.x {1.0}else{-1.0};let sy=if start.y<end.y {1.0}else{-1.0};
    let mut error=0.0_f32;let mut points=Vec::new();let mut i=0;
    while (i as f64)<dx as f64+dy as f64 {
        points.push((start.x.floor() as i32,start.y.floor() as i32));
        let up=error+dy;let down=error-dx;
        if up.abs()<down.abs() {start.x+=sx;error=up;}else{start.y+=sy;error=down;}
        i+=1;
    }
    points
}

pub fn circle(center:V2,radius:f32)->Vec<Point> {
    let mut x=radius.floor();let mut y=0.0_f32;let mut error=-x;let mut points=Vec::new();
    let scan=|points:&mut Vec<Point>,x:f32,y:f32|points.extend(line(V2::new(center.x-x,center.y+y),V2::new(center.x+x,center.y+y)));
    while x>=y {
        scan(&mut points,x,y);scan(&mut points,x,-y);
        if x!=y {scan(&mut points,y,x);scan(&mut points,y,-x);}
        error+=y;y+=1.0;error+=y;
        if error>=0.0 {error-=x;x-=1.0;error-=x;}
    }
    points
}

pub fn border(source:&[Point],radius:i32)->Vec<Point> {
    let mut source_set=HashSet::default();let ordered:Vec<_>=source.iter().copied().filter(|p|source_set.insert(*p)).collect();
    let mut seen=HashSet::default();let mut out=Vec::new();
    for (x,y) in ordered {for i in x-radius..=x+radius {for j in y-radius..=y+radius {
        let p=(i,j);if !source_set.contains(&p)&&seen.insert(p){out.push(p);}
    }}}
    out
}

pub fn rectangle(center:V2,width:f32,height:f32,rng:&mut SeededRandom)->Vec<Point> {
    let width=width.max(1.0);let height=height.max(1.0);
    let x0=(center.x as f64-width as f64/2.0) as i32;let x1=(center.x as f64+width as f64/2.0) as i32;
    let y0=(center.y as f64-height as f64/2.0) as i32;let y1=(center.y as f64+height as f64/2.0) as i32;
    let mut seen=HashSet::default();let mut out=Vec::new();
    let mut add=|p|{if seen.insert(p){out.push(p);}};
    let mut left=0.0_f32;let mut right=0.0_f32;
    for y in y0..y1 {
        left=(left+rng.random_range_f(-2.0,2.0)).clamp(-2.0,2.0);
        right=(right+rng.random_range_f(-2.0,2.0)).clamp(-2.0,2.0);
        let mut x=(x0 as f64-left as f64) as i32;
        while (x as f64)<x1 as f64+right as f64 {add((x,y));x+=1;}
    }
    let mut bottom=0.0_f32;let mut top=0.0_f32;
    for x in x0..x1 {
        bottom=(bottom+rng.random_range_f(-2.0,2.0)).clamp(-2.0,2.0);
        top=(top+rng.random_range_f(-2.0,2.0)).clamp(-2.0,2.0);
        for y in (y0 as f64-bottom as f64) as i32..y0 {add((x,y));}
        let mut y=y1;while (y as f64)<y1 as f64+top as f64 {add((x,y));y+=1;}
    }
    out
}

pub fn splat(center:V2,radius:f32,rng:&mut SeededRandom)->Vec<Point> {
    let count=(std::f32::consts::PI as f64*2.0*radius as f64) as f32;
    let count=count.round_ties_even() as i32;
    let mut seen=HashSet::default();let mut out=Vec::new();
    for i in 0..count {
        let random=rng.random_value();let length=(random as f64*random as f64*radius as f64) as f32;
        let angle=(std::f32::consts::PI as f64*2.0*(i as f64/count as f64)) as f32;
        let x=((angle as f64).sin() as f32*length) as f32;
        let y=((angle as f64).cos() as f32*length) as f32;
        for p in line(center,center.add(V2::new(x,y))) {if seen.insert(p) {out.push(p);}}
    }
    out
}

pub fn blob(center:V2,radius:f32,rng:&mut SeededRandom)->Vec<Point> {
    let mut controls=Vec::new();let mut seen=HashSet::default();
    let key=|v:V2|(if v.x==0.0 {0}else{v.x.to_bits()},if v.y==0.0 {0}else{v.y.to_bits()});
    let mut x=(radius+0.5).ceil() as i32;let mut y=0;let mut error=1-x;
    while x>=y {
        for (dx,dy) in [(x,y),(y,x),(-x,y),(-y,x),(-x,-y),(-y,-x),(x,-y),(y,-x)] {
            let p=center.add(V2::new(dx as f32,dy as f32));if seen.insert(key(p)) {controls.push(p);}
        }
        y+=1;if error<0 {error+=2*y+1;}else{x-=1;error+=2*(y-x)+1;}
    }
    crate::sampler::shuffle_seeded(&mut controls,rng);
    for p in &mut controls {
        let length=rng.next_double().sqrt();let angle=rng.next_double();
        *p=p.add(V2::new((length*angle.cos()) as f32,(length*angle.sin()) as f32).mul(radius));
    }
    let count=(2.0*radius as f64*radius as f64) as i32;
    let mut step=1.0/count as f32;let mut out=Vec::new();let mut points=HashSet::default();
    for i in 0..controls.len().saturating_sub(1) {
        let a=controls[i];let b=controls[i+1];
        let ta=if i==0 {b.sub(a)}else{b.sub(controls[i-1]).mul(0.5)};
        let tb=if i>=controls.len()-2 {b.sub(a)}else{controls[i+2].sub(a).mul(0.5)};
        if i==controls.len()-2 {step=(1.0/(count as f64-1.0)) as f32;}
        for j in 0..count {
            let t=(j as f32*step) as f64;
            let p=a.mul((2.0*t*t*t-3.0*t*t+1.0) as f32)
                .add(ta.mul((t*t*t-2.0*t*t+t) as f32))
                .add(b.mul((-2.0*t*t*t+3.0*t*t) as f32))
                .add(tb.mul((t*t*t-t*t) as f32));
            let point=(p.x as i32,p.y as i32);
            if points.insert(point) {out.push(point);}
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn line_excludes_endpoint_and_circle_keeps_repeated_scanlines() {
        assert_eq!(line(V2::ZERO,V2::new(2.0,0.0)),vec![(0,0),(1,0)]);
        assert_eq!(circle(V2::ZERO,1.0),vec![(-1,0),(0,0),(-1,0),(0,0)]);
        assert!(circle(V2::ZERO,0.0).is_empty());
    }
}
