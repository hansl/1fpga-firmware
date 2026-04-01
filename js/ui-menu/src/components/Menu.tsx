import * as React from 'react';

interface MenuItem {
  label: string;
  action: string;
}

interface MenuProps {
  items: MenuItem[];
  focused: number;
}

export function Menu({ items, focused }: MenuProps) {
  return (
    <view
      flexDirection="column"
      backgroundColor={[128, 0, 0]}
      opacity={0.85}
      padding={24}
      gap={8}
      borderRadius={8}
      alignItems="center"
    >
      {items.map((item, i) => (
        <view
          key={item.action}
          backgroundColor={i === focused ? [80, 80, 220] : undefined}
          padding={12}
          borderRadius={4}
          width={240}
          alignItems="center"
        >
          <text color={[255, 255, 255]} fontSize={24}>
            {item.label}
          </text>
        </view>
      ))}
    </view>
  );
}
