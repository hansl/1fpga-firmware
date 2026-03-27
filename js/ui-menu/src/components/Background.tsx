import * as React from 'react';

export function Background({ src }: { src: string }) {
  return (
    <image
      src={src}
      position="absolute"
      top={0}
      left={0}
      width={1920}
      height={1080}
    />
  );
}
