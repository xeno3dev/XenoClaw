import { useEffect, useRef } from 'react';

/**
 * Custom hook that calls a callback at a fixed interval.
 * The callback is always called with the latest closure values.
 * Pass `null` as delay to pause the interval.
 */
export function useInterval(callback: () => void, delay: number | null): void {
  const savedCallback = useRef(callback);

  // Remember the latest callback
  useEffect(() => {
    savedCallback.current = callback;
  }, [callback]);

  // Set up the interval
  useEffect(() => {
    if (delay === null) {
      return;
    }

    const id = setInterval(() => {
      savedCallback.current();
    }, delay);

    return () => clearInterval(id);
  }, [delay]);
}
