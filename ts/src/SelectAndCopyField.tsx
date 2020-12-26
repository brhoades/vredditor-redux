import React, { useRef, useState } from 'react';

import InputGroup from 'react-bootstrap/InputGroup';
import FormControl from 'react-bootstrap/FormControl';
import Button from 'react-bootstrap/Button';
import {
  GoClippy,
  GoCheck,
} from 'react-icons/go';

export default ({ value, displayValue, ...passthrough }: { value: string; displayValue?: boolean }) => {
  const [isLoading, setLoading] = useState<boolean>(false);
  const inputRef = useRef<HTMLInputElement>(null);
  const showInput = displayValue === undefined || displayValue;

  const onClick = (e: React.MouseEvent<HTMLButtonElement>) => {
    if (!inputRef?.current) {
      return;
    }

    setLoading(true);
    // hack around the browser needing to see it.
    if (!showInput) {
      inputRef.current.style.display = 'block';
    }
    inputRef.current.select();
    document.execCommand('copy');
    if (!showInput) {
      inputRef.current.style.display = 'none';
    }

    (e.target as any).focus();

    setTimeout(() => setLoading(false), 1000);
  };

  return (
    <>
      <InputGroup className="copy-group">
        <FormControl
          className="copy-only"
          onClick={() => inputRef.current?.select()}
          value={value}
          ref={inputRef}
          style={showInput ? {} : {
            display: 'none',
          }}
          readOnly
          {...passthrough}
        />
        <InputGroup.Append>
          <CopyButton isLoading={isLoading} onClick={onClick} />
        </InputGroup.Append>
      </InputGroup>
    </>
  );
};

type clk = (v: React.MouseEvent<HTMLButtonElement>) => void;

const CopyButton = ({ isLoading, onClick }: { isLoading: boolean; onClick: clk }) => (
  <Button
    onClick={onClick}
    variant="outline-success"
  >
    {isLoading ? (<GoCheck />) : (<GoClippy />)}
  </Button>
);
