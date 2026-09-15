import React from 'react';

interface ShellProps {
  children: React.ReactNode;
}

export const Shell: React.FC<ShellProps> = ({ children }) => {
  return (
    <div className="flex flex-col h-screen w-screen overflow-hidden bg-zinc-950/45 text-zinc-100 selection:bg-lime-400 selection:text-black">
      {/* Window titlebar drag region with centered title */}
      <header
        data-tauri-drag-region
        className="relative h-11 w-full flex items-center justify-center select-none shrink-0 border-b border-white/5 bg-black/25 backdrop-blur-xl"
      >
        <div className="absolute inset-x-0 flex items-center justify-center pointer-events-none">
          <span className="text-xs font-semibold tracking-[0.22em] text-zinc-300 uppercase">
            Sonora
          </span>
        </div>
      </header>

      {/* Workspace */}
      <div className="relative isolate ambient-wash flex-1 flex flex-col min-h-0 overflow-hidden">
        {children}
      </div>
    </div>
  );
};
