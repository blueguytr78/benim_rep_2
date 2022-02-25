import React, { useState } from 'react';
import { Button, Input, Header, Container } from 'semantic-ui-react';

const CreateAccount = ({ recoveryPhrase, sendPassword, endInitialConnectionPhase }) => {
  const [password, setPassword] = useState('');
  const [createdAccount, setCreatedAccount] = useState(false);

  const isValid = (password) => {
    console.log("[INFO]: Check password validity.")
    return password.length >= 8;
  };

  const onClickCreateAccount = async () => {
    console.log("[INFO]: Creating account.")
    if (isValid(password)) {
      await sendPassword(password);
      setPassword('');
      setCreatedAccount(true);
    }
  };

  const onClickConfirmRecoveryPhrase = async () => {
    console.log("[INFO]: Confirming recovery phrase.")
    await endInitialConnectionPhase();
  };

  return (
    <>
      {!createdAccount && (
        <>
          <Header> Create Account </Header>
          <Input
            type="password"
            label="Password"
            onChange={(e) => setPassword(e.target.value)}
          />
          <Button className="button" onClick={onClickCreateAccount}>
            Create Account
          </Button>
        </>
      )}
      {createdAccount && (
        <>
          <Header className="recovery-phrase-header">
            Your recovery phrase
          </Header>
          <div className="recovery-phrase-info">
            This phrase can restore your funds if you lose access to your
            account. Write it down on paper and store it somewhere secure. ⚠️
            Never share your recovery phrase with anyone!
          </div>
          <Container className="recovery-phrase-warning"></Container>
          <div className="recovery-phrase">
            <b>{recoveryPhrase}</b>
          </div>
          <Button className="button" onClick={onClickConfirmRecoveryPhrase}>
            I have written down my recovery phrase.
          </Button>
        </>
      )}
    </>
  );
};

export default CreateAccount;
